//! Phase 3 — interrupt infrastructure: PIC disable + LAPIC + IOAPIC + timer +
//! keyboard wire + STI.

use crate::boot::BootError;

pub fn init() -> Result<(), BootError> {
    let acpi = super::get_acpi_info();

    crate::pic::disable();

    crate::apic::lapic::init(acpi.lapic_base, crate::idt::VEC_SPURIOUS);
    crate::binfo!("intr", "LAPIC up base=0x{:X}", acpi.lapic_base);

    // Per-CPU bring-up for the BSP: set GS base so this_cpu() resolves via
    // gs:[0]. Called AFTER lapic::init so the APIC ID register is mapped.
    // AP cores are enumerated below (informational) but NOT started here.
    // init_bsp returns false on VMMs that silently ignore the GS-base MSR
    // (VirtualBox); this_cpu() then falls back to the BSP slot, so boot
    // continues on a single CPU regardless.
    let gs_ok = crate::cpu::init_bsp(0); // kernel_stack_top filled per-AP later
    crate::binfo!(
        "cpu", "cpu0 apic_id={} gs_base={}",
        crate::cpu::this_cpu().lapic_id,
        if gs_ok { "set" } else { "unavailable (BSP-slot fallback)" }
    );

    let n = acpi.cpus.len().max(1);
    crate::binfo!("cpu", "acpi: {} CPU(s) found ({} active, {} parked)", n, 1, n.saturating_sub(1));

    crate::apic::ioapic::init(acpi.ioapic_base);
    crate::binfo!("intr", "IOAPIC up base=0x{:X}", acpi.ioapic_base);

    let pm_timer = acpi.pm_timer_io.map(|port| (port, acpi.pm_timer_32bit));
    crate::timer::init(100, pm_timer)
        .map_err(|_| BootError::TimerInit("timer init failed"))?;
    crate::binfo!(
        "intr", "timer 100 Hz (ref={})",
        if pm_timer.is_some() { "acpi-pm" } else { "tsc" }
    );

    crate::keyboard::init(&acpi.overrides);
    crate::binfo!("intr", "keyboard IRQ1 wired overrides={}", acpi.overrides.len());

    crate::mouse::init(&acpi.overrides);
    crate::binfo!("intr", "mouse IRQ12 wired overrides={}", acpi.overrides.len());

    // Enable hardware interrupts.
    x86_64::instructions::interrupts::enable();
    crate::binfo!("intr", "STI — interrupts enabled");

    // Start the enumerated APs (Limine MpRequest) and park them idle.
    crate::smp::bringup();

    #[cfg(feature = "boot-checks")]
    {
        let ok = crate::memory::exec::self_test();
        crate::binfo!("mem", "exec W^X self-test {}", if ok { "ok" } else { "FAIL" });
        // wasm linear memory must read back zero (Mmap::reserve→make_accessible
        // path); regression for the egui font-atlas garble.
        let zi = crate::wasm::wt::platform::zero_init_self_test();
        crate::binfo!("wt", "linear-mem zero-init self-test {}", if zi { "ok" } else { "FAIL" });
        let mok = crate::mouse::self_test();
        crate::binfo!("mouse", "decode self-test {}", if mok { "ok" } else { "FAIL" });
        // Wasmtime no_std AOT runtime self-test (spike gate): run the embedded
        // hello.cwasm; its `run` export calls ruos.print(42).
        let wt = crate::wasm::wt::run_hello_demo();
        crate::binfo!("wt", "wasmtime AOT hello {}", if wt { "ok" } else { "FAIL" });
        // Real wasm32-wasip1 std binary via the WASI Linker (argv + fd_write).
        // (cat demo needs the VFS → runs in the fs phase, not here.)
        let ec = crate::wasm::wt::run_echo_demo();
        crate::binfo!("wt", "wasmtime WASI echo exit={}", ec);
        // Component Model bring-up: prove the no_std AOT component path runs.
        let cc = crate::wasm::wt::run_bringup_demo();
        crate::binfo!("wt", "component bringup run={}", cc);
        // Compositor GATE spike: a PERSISTENT reactor instance whose `frame()`
        // export is called 5× → tick==5. Proves the core multi-window mechanism.
        let rt = crate::wasm::wt::run_reactor_spike_demo();
        crate::binfo!("wm", "reactor spike frame-calls={}", rt);
    }

    Ok(())
}
