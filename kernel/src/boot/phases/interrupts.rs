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

    // Diagnostic: which cheap per-core-id primitives does this environment expose?
    crate::cpu::probe_fast_cpuid();

    #[cfg(feature = "boot-checks")]
    {
        crate::memory::allocbench::run_cpuid_bench();
        crate::memory::allocbench::run_single_core();
        crate::memory::allocbench::run_multicore();
    }

    // Step 2 boot-check: BSP→AP message round-trip. Proves targeted IPI delivery,
    // AP inbox drain, op execution, reply publish, and future resolution on the BSP.
    // Step 5 moved core 1 to `gui_worker_loop` (GuiCompositor); that loop does NOT
    // drain inboxes, so we target the first ComputeApp AP (core 2 on ≥3-CPU builds).
    // On 2-CPU builds (core 1 = GUI, no ComputeApp), the check is skipped.
    #[cfg(feature = "boot-checks")]
    {
        let target_ap = (0..crate::cpu::cpus_online())
            .find(|&c| c != 0 && crate::cpu::core_role(c) == crate::cpu::CoreRole::ComputeApp);
        if let Some(target) = target_ap {
            // op: sum the input bytes; run it on the target ComputeApp AP.
            fn sum_op(input: &[u8]) -> u64 { input.iter().map(|&b| b as u64).sum() }
            let input: alloc::boxed::Box<[u8]> = alloc::boxed::Box::from(&[1u8, 2, 3, 4][..]);
            let mut fut = crate::smp::inbox::request(target, sum_op, input);
            // Drive the future inline (no executor in this phase). A no-op waker is
            // fine — we poll in a bounded spin; the AP completes the reply async.
            use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
            fn noop(_: *const ()) {}
            fn clone_waker(_: *const ()) -> RawWaker { RawWaker::new(core::ptr::null(), &VT) }
            static VT: RawWakerVTable = RawWakerVTable::new(clone_waker, noop, noop, noop);
            let waker = unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &VT)) };
            let mut cx = Context::from_waker(&waker);
            let mut result = None;
            for _ in 0..50_000_000u64 {
                if let Poll::Ready(v) = core::future::Future::poll(core::pin::Pin::new(&mut fut), &mut cx) {
                    result = Some(v); break;
                }
                core::hint::spin_loop();
            }
            match result {
                Some(v) => crate::binfo!("inbox", "roundtrip ok core{} sum={} (expect 10)", target, v),
                None    => crate::binfo!("inbox", "roundtrip TIMEOUT"),
            }
        } else {
            crate::binfo!("inbox", "roundtrip skipped (no ComputeApp AP)");
        }
    }

    // Step 3a boot-check: verify AP1's LAPIC timer is firing at ~100 Hz.
    // We wait ~50 ms (5 BSP ticks at 100 Hz) and check that AP1's per-core
    // tick counter advanced by > 0 (expect ~5). Skipped on 1-core boots.
    #[cfg(feature = "boot-checks")]
    {
        if crate::cpu::cpus_online() >= 2 {
            let t0 = crate::timer::ap_ticks(1);
            let start = crate::timer::ticks();
            while crate::timer::ticks() < start + 5 { core::hint::spin_loop(); } // ~50 ms
            let grew = crate::timer::ap_ticks(1).saturating_sub(t0);
            crate::binfo!("timer", "ap1 ticks in 50ms = {} (expect > 0)", grew);
        } else {
            crate::binfo!("timer", "ap1 tick check skipped (1 core)");
        }
    }

    // Step 3b boot-check: verify a ComputeApp AP's per-core executor is polling
    // its heartbeat task (which uses per-core Delay + LAPIC timer). We wait
    // ~100 ms (10 BSP ticks) and check that the counter grew > 0 (expect ~5).
    // Core 1 = GuiCompositor (gui_worker_loop, no executor); the heartbeat_task
    // is spawned on core 2 (first ComputeApp AP) when ≥3 CPUs. Skipped on
    // ≤2-CPU boots where no ComputeApp AP exists.
    #[cfg(feature = "boot-checks")]
    {
        let has_compute_ap = (0..crate::cpu::cpus_online())
            .any(|c| c != 0 && crate::cpu::core_role(c) == crate::cpu::CoreRole::ComputeApp);
        if has_compute_ap {
            let h0 = crate::executor::HEARTBEAT.load(core::sync::atomic::Ordering::SeqCst);
            let start = crate::timer::ticks();
            while crate::timer::ticks() < start + 10 { core::hint::spin_loop(); } // ~100 ms
            let grew = crate::executor::HEARTBEAT
                .load(core::sync::atomic::Ordering::SeqCst)
                .saturating_sub(h0);
            crate::binfo!("exec", "compute-ap heartbeat ticks in 100ms = {} (expect ~5)", grew);
        } else {
            crate::binfo!("exec", "compute-ap heartbeat check skipped (no ComputeApp AP)");
        }
    }

    // Step 3c boot-check: BSP spawns a probe task onto a ComputeApp AP via
    // spawn_on(). Step 5 moved core 1 to `gui_worker_loop` (GuiCompositor);
    // that core never calls `run_core` so it has no executor / SendSpawner.
    // We target the FIRST ComputeApp AP (core 2 when ≥3 CPUs, else skip on
    // 2-CPU builds where only core 1 exists and it is the GUI core).
    #[cfg(feature = "boot-checks")]
    {
        // Find the first ComputeApp AP (has a real executor via run_core).
        let target_ap = (0..crate::cpu::cpus_online())
            .find(|&c| c != 0 && crate::cpu::core_role(c) == crate::cpu::CoreRole::ComputeApp);
        if let Some(target) = target_ap {
            // Retry until the target AP has published its SendSpawner (it enters
            // run_core during bringup; usually the very first attempt succeeds).
            let mut spawned = false;
            for _ in 0..1_000_000u64 {
                if crate::executor::spawn_on(target, crate::executor::cross_spawn_probe()).is_ok() {
                    spawned = true;
                    break;
                }
                core::hint::spin_loop();
            }
            // Wait for the probe to run on the target AP (woken via cross-core IPI).
            let mut ran_on = u32::MAX;
            for _ in 0..50_000_000u64 {
                let v = crate::executor::SPAWN_RAN_ON
                    .load(core::sync::atomic::Ordering::SeqCst);
                if v != u32::MAX { ran_on = v; break; }
                core::hint::spin_loop();
            }
            crate::binfo!(
                "exec",
                "cross-spawn ran_on=core{} (spawned={}, expect core{})",
                ran_on, spawned, target,
            );
        } else {
            // 1-CPU or 2-CPU (core 1 = GUI core, no ComputeApp AP): skip.
            crate::binfo!("exec", "cross-spawn skipped (no ComputeApp AP)");
        }
    }

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

        // RNG per-core distinct-streams check: draw a value on the BSP (core 0 →
        // RNG[0]) and one on a ComputeApp AP via the message bus; they must differ.
        // Core 1 = GuiCompositor (gui_worker_loop, no inbox drain) — must skip it.
        // Target: first ComputeApp AP. Skipped if none exists (≤2-CPU boots).
        {
            fn rng_draw(_in: &[u8]) -> u64 { crate::rng::next_u64() }
            let target_ap = (0..crate::cpu::cpus_online())
                .find(|&c| c != 0 && crate::cpu::core_role(c) == crate::cpu::CoreRole::ComputeApp);
            if let Some(target) = target_ap {
                let bsp = crate::rng::next_u64();
                let mut fut = crate::smp::inbox::request(target, rng_draw, alloc::boxed::Box::from(&[][..]));
                use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
                fn noop(_: *const ()) {}
                fn cl(_: *const ()) -> RawWaker { RawWaker::new(core::ptr::null(), &RNG_VT) }
                static RNG_VT: RawWakerVTable = RawWakerVTable::new(cl, noop, noop, noop);
                let w = unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &RNG_VT)) };
                let mut cx = Context::from_waker(&w);
                let mut ap: Option<u64> = None;
                for _ in 0..50_000_000u64 {
                    if let Poll::Ready(v) = core::future::Future::poll(
                        core::pin::Pin::new(&mut fut), &mut cx,
                    ) {
                        ap = Some(v);
                        break;
                    }
                    core::hint::spin_loop();
                }
                crate::binfo!("rng", "percore distinct bsp!=ap{} -> {}",
                    target, ap.map_or(false, |a| a != bsp));
            } else {
                crate::binfo!("rng", "percore distinct skipped (no ComputeApp AP)");
            }
        }
    }

    Ok(())
}
