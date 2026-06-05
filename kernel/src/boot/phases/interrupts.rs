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
        let umok = crate::usb::mouse::self_test();
        crate::binfo!("usb", "boot-mouse decode self-test {}", if umok { "ok" } else { "FAIL" });
        let usk = crate::usb::usage::scancode_self_test();
        crate::binfo!("usb", "usage->scancode self-test {}", if usk { "ok" } else { "FAIL" });
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
        // export is called 5× → tick==5. Also verifies the committed surface
        // buffer (commit_b0==0x05, pixels==307200) arrives intact in the kernel.
        let (calls, b0, plen) = crate::wasm::wt::run_reactor_spike_demo();
        crate::binfo!("wm", "reactor spike calls={} commit_b0=0x{:02X} pixels={}", calls, b0, plen);
        // SP3 window-manager pure-logic selftest: decoration geometry + hit-test
        // + z-order raise + drag math, NO wasm instances (fast + deterministic).
        let wmf = crate::wasm::wt::run_wm_logic_selftest();
        crate::binfo!("wm", "sp3 logic selftest flags=0b{:05b}", wmf);
        // SP5 launcher registry: N launchable apps, all deserialise to a Module.
        let (apps, mods_ok) = crate::wasm::wt::run_registry_demo();
        crate::binfo!("wm", "launcher registry apps={} modules_ok={}", apps, mods_ok);
        // SP5 lifecycle: spawn the self-closing app, run frame()+reap rounds, and
        // confirm it tears itself down (final_live==0) + the id is recycled.
        let (sp, peak, fin) = crate::wasm::wt::run_lifecycle_demo();
        crate::binfo!("wm", "lifecycle spawns={} peak_live={} final_live={}", sp, peak, fin);
        // SP-A: spawn the wasip1 STD probe as a window + drive one frame. A
        // non-zero pixels=307200 proves a std/wasip1 guest instantiates + runs
        // its std heap alloc + commits against the unified Linker<AppState>.
        let pn = crate::wasm::wt::run_wasip1_probe_demo();
        crate::binfo!("wm", "wasip1 probe spawn ok pixels={}", pn);
        // SP-B: spawn the egui CSD demo as a window + drive one frame. A non-zero
        // pixels=614400 (480×320×4) proves the egui guest instantiated against the
        // unified Linker<AppState>, ran one egui ctx.run + tessellate + raster, and
        // committed its surface — SP-B's core success. egui's first frame seeds an
        // ahash HashMap via WASI `random_get`, so the CSPRNG must be live: seed it
        // now (idempotent — `userland::init()` re-calls and no-ops). RDRAND is the
        // only dependency and it's available this early.
        crate::rng::init();
        let en = crate::wasm::wt::run_egui_demo_demo();
        crate::binfo!("wm", "egui demo spawn ok pixels={}", en);
        // SP-C: prove the wm.spawn deferred-spawn mechanism grows the window list
        // to 2 (bit0) AND the wm.set_background mechanism forces a window to the
        // full framebuffer (bit1). Embedded module (VFS /bin not mounted yet); the
        // real VFS wm.spawn is covered visually. Expect flags=0b11.
        let spc = crate::wasm::wt::run_spc_demo();
        crate::binfo!("wm", "spc flags=0b{:02b}", spc);
        // SP-D: prove the desktop-shell boot wiring headlessly — the new
        // wm.poweroff/wm.surface_size host fns register (the empty compositor
        // builds past add_to_linker) AND the wm.set_background full-screen
        // mechanism the shell self-flags with still pins a window to the whole
        // framebuffer. Returns the forced bg size packed (w<<16)|h. The shell
        // booting AS the bg desktop (+ launcher → wm.spawn) is verified visually.
        let spd = crate::wasm::wt::run_spd_demo();
        crate::binfo!("wm", "spd: hostfns ok bg={}x{}", spd >> 16, spd & 0xffff);
    }

    Ok(())
}
