//! USB device enumeration: root-port scan/reset, then (later tasks) slot
//! allocation, addressing, descriptors.
use crate::usb::xhci::Xhci;

/// A reset, connected root port ready for enumeration.
pub struct PortInfo {
    pub port:  u8,   // 1-based port number
    pub speed: u8,   // PSI value (1=Full, 2=Low, 3=High, 4=Super)
}

/// Scan all root ports; reset each connected one and read its speed. Returns the
/// first connected+reset port (MVP enumerates one device). Logs each.
///
/// # PORTSC RW1C note
/// `update_volatile_at` does a plain read-modify-write. PORTSC contains several
/// Read-Write-1-to-Clear (RW1C) status-change bits; to avoid accidentally
/// clearing them during the set_port_reset() write, we call `set_0_*` on every
/// RW1C change bit in the same closure so they stay 0 in the written value.
/// Only when we deliberately clear PRC do we call `clear_port_reset_change()`.
pub fn scan_ports(x: &mut Xhci) -> Option<PortInfo> {
    let mut found = None;

    for port in 1..=x.max_ports {
        let idx = (port - 1) as usize;

        // Check if a device is connected (CCS = Current Connect Status).
        let p = x.regs.port_register_set.read_volatile_at(idx);
        if !p.portsc.current_connect_status() {
            continue;
        }

        // Assert port reset (PR bit, RW1S). Preserve all RW1C change bits by
        // writing 0 to them so the read-modify-write does not accidentally clear
        // them (writing 1 to a RW1C bit clears it in hardware).
        x.regs.port_register_set.update_volatile_at(idx, |p| {
            p.portsc.set_port_reset();
            // Write 0 to all RW1C change bits to avoid clearing them.
            p.portsc.set_0_port_enabled_disabled();
            p.portsc.set_0_connect_status_change();
            p.portsc.set_0_port_enabled_disabled_change();
            p.portsc.set_0_warm_port_reset_change();
            p.portsc.set_0_over_current_change();
            p.portsc.set_0_port_reset_change();
            p.portsc.set_0_port_link_state_change();
            p.portsc.set_0_port_config_error_change();
        });

        // Wait (bounded 50 ms) for reset to complete — PRC (Port Reset Change) set.
        let start = crate::boot::clock::elapsed_ms();
        let mut reset_done = false;
        while crate::boot::clock::elapsed_ms() - start < 50 {
            let p = x.regs.port_register_set.read_volatile_at(idx);
            if p.portsc.port_reset_change() {
                reset_done = true;
                break;
            }
            core::hint::spin_loop();
        }

        // Clear PRC (RW1C) and preserve all other RW1C change bits.
        x.regs.port_register_set.update_volatile_at(idx, |p| {
            p.portsc.clear_port_reset_change();   // write 1 → hardware clears PRC
            // Write 0 to all other RW1C change bits so we don't clear them.
            p.portsc.set_0_port_enabled_disabled();
            p.portsc.set_0_connect_status_change();
            p.portsc.set_0_port_enabled_disabled_change();
            p.portsc.set_0_warm_port_reset_change();
            p.portsc.set_0_over_current_change();
            p.portsc.set_0_port_link_state_change();
            p.portsc.set_0_port_config_error_change();
        });

        let p = x.regs.port_register_set.read_volatile_at(idx);
        let speed   = p.portsc.port_speed();
        let enabled = p.portsc.port_enabled_disabled();

        crate::binfo!(
            "usb",
            "port {} connected speed={} enabled={} reset_done={}",
            port, speed, enabled, reset_done
        );

        if found.is_none() && enabled {
            found = Some(PortInfo { port, speed });
        }
    }

    if found.is_none() {
        crate::bwarn!("usb", "no usable port after scan");
    }
    found
}
