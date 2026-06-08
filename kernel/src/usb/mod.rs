//! USB stack: xHCI controller + HID keyboard. Polled (no MSI). See
//! docs/superpowers/specs/2026-06-01-usb-xhci-hid-design.md.
pub mod control;
pub mod device;
pub mod encoding;
pub mod hid;
pub mod msc;
pub mod mouse;
pub mod hub;
pub mod registry;
pub mod usage;
pub mod xhci;

use crate::sync::IrqMutex;
use spin::Once;
use alloc::vec::Vec;

/// All xHCI controllers, set once during `init()`. A machine can have several
/// (e.g. Thunderbolt xHCI + PCH xHCI); each is tracked by its index in this Vec
/// (`Xhci::idx`). Per-device state lives in `registry`, keyed by `(ctrl, slot)`.
pub(crate) static CTRLS: Once<IrqMutex<Vec<xhci::Xhci>>> = Once::new();

/// Bring up the xHCI controller and enumerate devices. Non-fatal: logs and
/// returns if there is no controller or bring-up fails.
pub fn init() {
    xhci::init();
}

/// Drain the event ring + process HID input. Called by `usb_poll_task`.
pub fn poll() {
    xhci::poll();
}

/// Per-root-port PORTSC snapshot `(ctrl, port, ccs, ped, pls, pp, speed)` across
/// ALL controllers, for boot diagnostics on hardware without serial (non-cfg;
/// `probe_ports` is gated). `ccs`=connected, `ped`=enabled, `pls`=link state,
/// `pp`=powered.
pub fn dump_ports() -> alloc::vec::Vec<(u8, u8, bool, bool, u8, bool, u8)> {
    let mut v = alloc::vec::Vec::new();
    let cell = match CTRLS.get() { Some(c) => c, None => return v };
    let mut g = cell.lock();
    for x in g.iter_mut() {
        for port in 1..=x.max_ports {
            let p = x.regs.port_register_set.read_volatile_at((port - 1) as usize);
            v.push((
                x.idx,
                port,
                p.portsc.current_connect_status(),
                p.portsc.port_enabled_disabled(),
                p.portsc.port_link_state(),
                p.portsc.port_power(),
                p.portsc.port_speed(),
            ));
        }
    }
    v
}

/// Re-seed the enumeration worklist for every root port that is currently
/// connected but not yet enumerated. `usb::init` seeds connected ports ONCE at
/// controller start; on real hardware a device (e.g. the boot USB stick) can
/// read disconnected at that instant (USB3 link training / USB2 debounce) and,
/// if its Port-Status-Change event was missed, never gets enumerated. The
/// `media_bin` pump calls this each iteration so a late/missed connect is still
/// picked up. No-op without a controller.
pub fn reseed_connected_ports() {
    let cell = match CTRLS.get() { Some(c) => c, None => return };
    let mut g = cell.lock();
    for x in g.iter_mut() {
        for port in 1..=x.max_ports {
            let connected = x.regs.port_register_set
                .read_volatile_at((port - 1) as usize)
                .portsc.current_connect_status();
            if connected && crate::usb::registry::find_root(x.idx, port).is_none() {
                crate::usb::registry::push_action(
                    crate::usb::registry::UsbAction::RootPortChanged { ctrl: x.idx, port },
                );
            }
        }
    }
}

/// One connected root port's decoded PORTSC, for the `usb-probe` summary.
/// Fields: `(port, ped, pr, pls, pp, speed)`. `speed` PSI: 1=Full 2=Low 3=High
/// 4=Super. `pls`: 0=U0(enabled) 4=Disabled 5=RxDetect 6=Inactive 7=Polling.
#[cfg(feature = "usb-probe")]
pub struct ProbePort {
    pub port: u8,
    pub ped: bool,
    pub pr: bool,
    pub pls: u8,
    pub pp: bool,
    pub speed: u8,
}

/// Diagnostic snapshot of the **connected** root ports (disconnected ports are
/// skipped to keep the frozen summary on one screen on real controllers, which
/// can expose 20+ ports). Read once, after enumeration has been drained, so the
/// PED/PLS shown is the post-reset state.
#[cfg(feature = "usb-probe")]
pub fn probe_ports() -> alloc::vec::Vec<ProbePort> {
    let mut v = alloc::vec::Vec::new();
    let cell = match CTRLS.get() { Some(c) => c, None => return v };
    let mut g = cell.lock();
    for x in g.iter_mut() {
        for port in 1..=x.max_ports {
            let p = x.regs.port_register_set.read_volatile_at((port - 1) as usize);
            if !p.portsc.current_connect_status() { continue; }
            v.push(ProbePort {
                port,
                ped: p.portsc.port_enabled_disabled(),
                pr: p.portsc.port_reset(),
                pls: p.portsc.port_link_state(),
                pp: p.portsc.port_power(),
                speed: p.portsc.port_speed(),
            });
        }
    }
    v
}
