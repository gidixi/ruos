//! Phase — USB: bring up the xHCI controller + enumerate HID devices. Runs
//! after `pci` (needs the xHCI BAR). Non-fatal: a machine without xHCI boots
//! fine (USB is additive to the PS/2 keyboard).
use crate::boot::BootError;

pub fn init() -> Result<(), BootError> {
    crate::usb::init();
    #[cfg(feature = "usb-probe")]
    probe();
    Ok(())
}

/// Diagnostic (feature `usb-probe`): drive USB enumeration to completion HERE —
/// synchronously, while the framebuffer console is still text and at INFO level
/// (before userland raises the threshold to WARN and the GUI claims the
/// framebuffer) — then print a one-screen device summary and HALT so a machine
/// with no serial can be photographed. This tells us WHERE the real-hardware
/// path diverges: a USB keyboard that never appears (enumeration/HID-dispatch
/// failed) vs. one that appears as `Keyboard` at Full/Low speed (enumerated, but
/// the interrupt-endpoint interval encoding likely starves its reports).
#[cfg(feature = "usb-probe")]
fn probe() {
    use crate::boot::clock::elapsed_ms;

    crate::kprintln!("=== USB PROBE: draining enumeration (3s) ===");
    // Enumeration runs on `usb::poll()`; in normal boot that is the async
    // `usb_poll_task`. Here we pump it directly until the worklist is quiet.
    let deadline = elapsed_ms() + 3000;
    while elapsed_ms() < deadline {
        crate::usb::poll();
        core::hint::spin_loop();
    }

    crate::kprintln!("=== USB PROBE: connected ports (post-reset) ===");
    let ports = crate::usb::probe_ports();
    if ports.is_empty() {
        crate::kprintln!("  (no ports connected)");
    } else {
        for p in ports {
            crate::kprintln!(
                "  port {} ped={} pr={} pls={} pp={} speed={} ({})",
                p.port, p.ped, p.pr, p.pls, p.pp, p.speed, speed_name(p.speed)
            );
        }
    }

    crate::kprintln!("=== USB PROBE: enumerated slots ===");
    let slots = crate::usb::registry::probe_dump();
    if slots.is_empty() {
        crate::kprintln!("  (none enumerated)");
    } else {
        for (slot, root_port, speed, kind) in slots {
            crate::kprintln!(
                "  slot {} port {} speed {} ({}) kind={}",
                slot, root_port, speed, speed_name(speed), kind
            );
        }
    }

    crate::kprintln!(
        "=== USB PROBE: mouse events injected = {} ===",
        crate::mouse::event_count()
    );

    crate::kprintln!("=== USB PROBE: halted — photograph this screen ===");
    loop {
        x86_64::instructions::interrupts::disable();
        x86_64::instructions::hlt();
    }
}

#[cfg(feature = "usb-probe")]
fn speed_name(psi: u8) -> &'static str {
    match psi {
        1 => "Full",
        2 => "Low",
        3 => "High",
        4 => "Super",
        _ => "?",
    }
}
