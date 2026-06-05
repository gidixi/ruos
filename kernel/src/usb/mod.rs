//! USB stack: xHCI controller + HID keyboard. Polled (no MSI). See
//! docs/superpowers/specs/2026-06-01-usb-xhci-hid-design.md.
pub mod control;
pub mod device;
pub mod encoding;
pub mod hid;
pub mod mouse;
pub mod hub;
pub mod registry;
pub mod usage;
pub mod xhci;

use crate::sync::IrqMutex;
use spin::Once;

/// Global xHCI controller handle, set once during `init()`. Per-device state now
/// lives in `registry` (keyed by slot), not in singletons — hot-plug needs N
/// devices, and the worklist drives enumeration through `registry::insert`.
pub(crate) static CTRL: Once<IrqMutex<Option<xhci::Xhci>>> = Once::new();

/// Bring up the xHCI controller and enumerate devices. Non-fatal: logs and
/// returns if there is no controller or bring-up fails.
pub fn init() {
    xhci::init();
}

/// Drain the event ring + process HID input. Called by `usb_poll_task`.
pub fn poll() {
    xhci::poll();
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
    let cell = match CTRL.get() { Some(c) => c, None => return v };
    let mut g = cell.lock();
    let x = match g.as_mut() { Some(x) => x, None => return v };
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
    v
}
