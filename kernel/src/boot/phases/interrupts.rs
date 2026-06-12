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
        // Demand paging: a reserved-but-untouched range must cost ZERO frames,
        // and a touch must commit exactly its page lazily (the OOM fix).
        let dp = crate::wasm::wt::demand::self_test();
        crate::binfo!("wt", "linear-mem demand-paging self-test {}", if dp { "ok" } else { "FAIL" });
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
        // MT Fase 2 gate 1: SharedMemory + atomics nativi (fork wasmtime no_std).
        let g1 = crate::wasm::wt::run_threads_gate1();
        crate::binfo!("wt", "THREADS-OK 1 = {}", if g1 { "ok" } else { "FAIL" });
        // MT Fase 2 fiber self-test: host-only fiber suspend/resume cross-core
        // (con ≥3 core ran_on/resumed devono essere core ComputeApp, cioè ≥2).
        let (fok, fran, fres) = crate::wasm::wt::threads::fiber_self_test();
        crate::binfo!(
            "wt",
            "THREADS-FIBER-OK = {} ran_on={} resumed_on={}",
            if fok { "ok" } else { "FAIL" }, fran, fres,
        );
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
        // (The Blitz GATE + embedded-viewer boot-checks were retired with the
        // boot-check cleanup — changelog 456. Their 55+77 MB blobs exhausted the
        // frame allocator under -m 1024; the GATE numbers are archived in
        // CHANGELOG 425 / ruos-test GATE.md, and the real viewer ships in apps/.)

        // Epoch watchdog: the deliberately-spinning reactor must be trapped
        // (frame() WATCHDOG in the log) + reaped while a healthy reactor keeps
        // ticking — a runaway guest can no longer freeze the compositor loop.
        let (wd_reaped, wd_tick) = crate::wasm::wt::run_watchdog_demo();
        crate::binfo!("wm", "epoch watchdog spinner_reaped={} healthy_tick={}", wd_reaped, wd_tick);

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

        // Step 3d boot-check: no-fault remap test — proves cross-core TLB shootdown
        // actually invalidates stale entries on other cores.
        //
        // Protocol:
        //   1. Map test_virt → frame_A; write sentinel 0xAAAAAAAA into frame_A's HHDM alias.
        //   2. AP reads test_virt → r1 (caches the TLB translation A in its TLB).
        //   3. unmap(test_virt) [fires shootdown] then map(test_virt → frame_B) [no shootdown].
        //   4. AP reads test_virt → r2.
        //   r1=0xAAAAAAAA AND r2=0xBBBBBBBB → shootdown flushed the stale entry. PROOF.
        //   r2=0xAAAAAAAA → stale TLB entry survived (shootdown broken). FAIL.
        //
        // We target the first ComputeApp AP (core 2 when ≥3 CPUs). Skipped if none exists.
        {
            use x86_64::VirtAddr;
            use x86_64::structures::paging::PageTableFlags;

            // A dedicated virt range that does not collide with any existing test mapping.
            const TEST_VIRT: u64 = 0x4444_0000_0000;

            let target_ap = (0..crate::cpu::cpus_online())
                .find(|&c| c != 0 && crate::cpu::core_role(c) == crate::cpu::CoreRole::ComputeApp);

            if let Some(target) = target_ap {
                // Allocate two physical frames.
                let frame_a = crate::memory::allocate_frame()
                    .expect("tlb-test: failed to allocate frame A");
                let frame_b = crate::memory::allocate_frame()
                    .expect("tlb-test: failed to allocate frame B");

                // Write sentinels into each frame via their HHDM alias.
                let va_a = crate::memory::hhdm_virt(frame_a.start_address());
                let va_b = crate::memory::hhdm_virt(frame_b.start_address());
                unsafe {
                    core::ptr::write_volatile(va_a.as_mut_ptr::<u32>(), 0xAAAAAAAAu32);
                    core::ptr::write_volatile(va_b.as_mut_ptr::<u32>(), 0xBBBBBBBBu32);
                }

                let flags = PageTableFlags::PRESENT
                    | PageTableFlags::WRITABLE
                    | PageTableFlags::NO_EXECUTE;

                // Step 1: map test_virt → frame_A.
                crate::memory::map_page(
                    VirtAddr::new(TEST_VIRT),
                    frame_a.start_address(),
                    flags,
                ).expect("tlb-test: map frame_A failed");

                // Step 2: AP reads test_virt → r1 (caches translation A in its TLB).
                fn read_virt(_input: &[u8]) -> u64 {
                    unsafe { core::ptr::read_volatile(TEST_VIRT as *const u32) as u64 }
                }
                let mut fut1 = crate::smp::inbox::request(
                    target,
                    read_virt,
                    alloc::boxed::Box::from(&[][..]),
                );
                use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
                fn tlb_noop(_: *const ()) {}
                fn tlb_clone(_: *const ()) -> RawWaker {
                    RawWaker::new(core::ptr::null(), &TLB_VT)
                }
                static TLB_VT: RawWakerVTable =
                    RawWakerVTable::new(tlb_clone, tlb_noop, tlb_noop, tlb_noop);
                let waker = unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &TLB_VT)) };
                let mut cx = Context::from_waker(&waker);
                let mut r1: Option<u64> = None;
                for _ in 0..50_000_000u64 {
                    if let Poll::Ready(v) = core::future::Future::poll(
                        core::pin::Pin::new(&mut fut1), &mut cx,
                    ) {
                        r1 = Some(v);
                        break;
                    }
                    core::hint::spin_loop();
                }
                let r1 = r1.unwrap_or(0xDEAD);

                // Step 3: unmap (fires shootdown → invalidates AP's cached entry),
                // then remap to frame_B (map_page: no shootdown needed for new mapping).
                let _ = crate::memory::unmap_page(VirtAddr::new(TEST_VIRT));
                crate::memory::map_page(
                    VirtAddr::new(TEST_VIRT),
                    frame_b.start_address(),
                    flags,
                ).expect("tlb-test: map frame_B failed");

                // Step 4: AP reads test_virt → r2.  Must see 0xBBBBBBBB (frame_B),
                // not 0xAAAAAAAA (stale frame_A entry), proving the shootdown worked.
                let mut fut2 = crate::smp::inbox::request(
                    target,
                    read_virt,
                    alloc::boxed::Box::from(&[][..]),
                );
                let mut r2: Option<u64> = None;
                for _ in 0..50_000_000u64 {
                    if let Poll::Ready(v) = core::future::Future::poll(
                        core::pin::Pin::new(&mut fut2), &mut cx,
                    ) {
                        r2 = Some(v);
                        break;
                    }
                    core::hint::spin_loop();
                }
                let r2 = r2.unwrap_or(0xDEAD);

                // Clean up: unmap + free both frames.
                let _ = crate::memory::unmap_page(VirtAddr::new(TEST_VIRT));
                crate::memory::free_frame(frame_a);
                crate::memory::free_frame(frame_b);

                crate::binfo!(
                    "tlb",
                    "remap seen by ap: r1=0x{:X} r2=0x{:X} shootdown_ok={}",
                    r1,
                    r2,
                    r1 == 0xAAAAAAAA && r2 == 0xBBBBBBBB
                );
            } else {
                crate::binfo!("tlb", "remap test skipped (no ComputeApp AP)");
            }
        }

        // Range-shootdown boot-check: 64 pages flagged via set_flags_range must
        // cost ONE shootdown (not 64 — the batch fix), then unmap_range must
        // remove all 64 translations (re-unmap of first/last page → NotMapped).
        //
        // REMOTE leg (when a ComputeApp AP exists) — same protocol as the Step 3d
        // remap test above, but through the RANGE path: the AP reads the FIRST
        // page BEFORE unmap_range (caching the translation in its TLB) → r1.
        // unmap_range of all 64 pages fires ONE shootdown with 64 > FLUSH_THRESHOLD
        // → remote cores take the CR3-reload path. Remapping the first page to a
        // NEW frame needs no shootdown (map_page over not-present — x86 caches no
        // negative entries), so the AP's re-read → r2 can only see the new sentinel
        // if the CR3 reload actually dropped its cached entry.
        //   r1=0xAAAAAAAA AND r2=0xBBBBBBBB → remote CR3 flush proven.
        //   r2=0xAAAAAAAA → stale entry survived the range shootdown. FAIL.
        {
            use x86_64::VirtAddr;
            use x86_64::structures::paging::PageTableFlags;

            const RANGE_VIRT: u64 = 0x4445_0000_0000; // disjoint from other tests
            const N: usize = 64; // > FLUSH_THRESHOLD → exercises the CR3-reload path
            let rw = PageTableFlags::PRESENT
                | PageTableFlags::WRITABLE
                | PageTableFlags::NO_EXECUTE;
            let ro = PageTableFlags::PRESENT | PageTableFlags::NO_EXECUTE;

            let mut mapped = 0usize;
            for i in 0..N {
                let Some(f) = crate::memory::allocate_frame() else { break };
                let va = VirtAddr::new(RANGE_VIRT + (i as u64) * 0x1000);
                if crate::memory::map_page(va, f.start_address(), rw).is_err() {
                    crate::memory::free_frame(f);
                    break;
                }
                mapped += 1;
            }

            // Sentinel into the FIRST page (still RW here) for the remote read.
            if mapped > 0 {
                // SAFETY: just mapped PRESENT|WRITABLE to a private test frame.
                unsafe { core::ptr::write_volatile(RANGE_VIRT as *mut u32, 0xAAAA_AAAAu32); }
            }

            let (sd0, _) = crate::memory::tlb::stats();
            let changed = crate::memory::set_flags_range(VirtAddr::new(RANGE_VIRT), mapped, ro)
                .unwrap_or(usize::MAX);
            let (sd1, _) = crate::memory::tlb::stats();
            let sd_grew = sd1.saturating_sub(sd0);

            // Inline driver for the AP read (same no-op-waker bounded poll the
            // Step 3d remap test uses).
            fn read_first(_input: &[u8]) -> u64 {
                // SAFETY: RANGE_VIRT is mapped (RO or RW) whenever this op runs.
                unsafe { core::ptr::read_volatile(RANGE_VIRT as *const u32) as u64 }
            }
            fn poll_reply(mut fut: crate::smp::inbox::ReplyFuture) -> Option<u64> {
                use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
                fn nop(_: *const ()) {}
                fn cl(_: *const ()) -> RawWaker { RawWaker::new(core::ptr::null(), &VT) }
                static VT: RawWakerVTable = RawWakerVTable::new(cl, nop, nop, nop);
                let waker = unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &VT)) };
                let mut cx = Context::from_waker(&waker);
                for _ in 0..50_000_000u64 {
                    if let Poll::Ready(v) = core::future::Future::poll(
                        core::pin::Pin::new(&mut fut), &mut cx,
                    ) {
                        return Some(v);
                    }
                    core::hint::spin_loop();
                }
                None
            }

            // Remote leg part 1 — BEFORE unmap_range: the AP reads the first page,
            // caching the soon-to-be-stale translation in its TLB. Skipped when no
            // ComputeApp AP is online (single-core / 2-CPU GUI-only boots) or when
            // the range failed to map (nothing safe to read).
            let target_ap = (0..crate::cpu::cpus_online())
                .find(|&c| c != 0 && crate::cpu::core_role(c) == crate::cpu::CoreRole::ComputeApp)
                .filter(|_| mapped > 0);
            let r1 = target_ap.map(|t| {
                poll_reply(crate::smp::inbox::request(
                    t, read_first, alloc::boxed::Box::from(&[][..]),
                )).unwrap_or(0xDEAD)
            });

            // ONE range shootdown for all 64 pages (> FLUSH_THRESHOLD → remote CR3).
            let unmapped = crate::memory::unmap_range(VirtAddr::new(RANGE_VIRT), mapped);
            // Translations gone: re-unmapping the first and last page must fail.
            // (Probed BEFORE the remote remap below re-populates the first page.)
            let last = RANGE_VIRT + (mapped.max(1) as u64 - 1) * 0x1000;
            let gone = crate::memory::unmap_page(VirtAddr::new(RANGE_VIRT)).is_err()
                && crate::memory::unmap_page(VirtAddr::new(last)).is_err();

            // Remote leg part 2 — remap the first page to a NEW frame holding a
            // different sentinel (map_page: no shootdown needed), AP re-reads → r2.
            let mut r2: Option<u64> = None;
            let mut remote_ok = true; // no AP → vacuously true (local check only)
            if let Some(t) = target_ap {
                let frame_b = crate::memory::allocate_frame()
                    .expect("tlb-range-test: failed to allocate frame B");
                // SAFETY: HHDM alias of a freshly allocated frame is kernel-writable.
                unsafe {
                    core::ptr::write_volatile(
                        crate::memory::hhdm_virt(frame_b.start_address()).as_mut_ptr::<u32>(),
                        0xBBBB_BBBBu32,
                    );
                }
                crate::memory::map_page(VirtAddr::new(RANGE_VIRT), frame_b.start_address(), rw)
                    .expect("tlb-range-test: remap frame B failed");
                let v2 = poll_reply(crate::smp::inbox::request(
                    t, read_first, alloc::boxed::Box::from(&[][..]),
                )).unwrap_or(0xDEAD);
                r2 = Some(v2);
                remote_ok = r1 == Some(0xAAAA_AAAA) && v2 == 0xBBBB_BBBB;
                // Cleanup of the remote leg: unmap + free the new frame.
                let _ = crate::memory::unmap_page(VirtAddr::new(RANGE_VIRT));
                crate::memory::free_frame(frame_b);
            }

            // sd_grew: ≥ 1 (set_flags_range batched the broadcast) and < 64 (the
            // per-page storm is gone). NOT == 1: this boot phase is assumed
            // quiescent, but the counters are GLOBAL — a future concurrent
            // shootdown from another core during the window would bump them, and
            // the check must not be fragile to that.
            let ok = mapped == N && changed == N && sd_grew >= 1 && sd_grew < 64
                && unmapped == N && gone && remote_ok;
            match (r1, r2) {
                (Some(r1), Some(r2)) => crate::binfo!(
                    "tlb",
                    "range check {}: mapped={} changed={} shootdowns+={} unmapped={} gone={} remote_r1=0x{:X} r2=0x{:X}",
                    if ok { "ok" } else { "FAIL" },
                    mapped, changed, sd_grew, unmapped, gone, r1, r2
                ),
                _ => {
                    crate::binfo!("tlb", "range remote check skipped (no AP)");
                    crate::binfo!(
                        "tlb",
                        "range check {}: mapped={} changed={} shootdowns+={} unmapped={} gone={}",
                        if ok { "ok" } else { "FAIL" },
                        mapped, changed, sd_grew, unmapped, gone
                    );
                }
            }
        }

        // C1 boot-check: spawn `wasm_ap_probe` onto core 2 (a ComputeApp core)
        // and wait for it to complete. The probe calls `run_hello_demo()` (the
        // embedded hello.cwasm via wasmtime AOT) ON THAT CORE — proving the
        // runtime instantiates + executes correctly off the BSP.
        //
        // Skipped if there are fewer than 3 CPUs (core 2 does not exist) or
        // core 2 is not a ComputeApp core (unexpected role assignment).
        {
            if crate::cpu::cpus_online() >= 3
                && crate::cpu::core_role(2) == crate::cpu::CoreRole::ComputeApp
            {
                // Retry until core 2 has published its SendSpawner.
                let mut spawned = false;
                for _ in 0..1_000_000u64 {
                    if crate::executor::spawn_on(2, crate::executor::wasm_ap_probe()).is_ok() {
                        spawned = true;
                        break;
                    }
                    core::hint::spin_loop();
                }
                // Wait for the probe to finish (wasmtime instantiation is sync +
                // heavier than the other probes — give it a generous spin budget).
                let mut ok: u32 = 2;
                let mut ran: u32 = u32::MAX;
                for _ in 0..200_000_000u64 {
                    let o = crate::executor::WASM_AP_OK
                        .load(core::sync::atomic::Ordering::SeqCst);
                    if o != 2 {
                        ok  = o;
                        ran = crate::executor::WASM_AP_RAN_ON
                            .load(core::sync::atomic::Ordering::SeqCst);
                        break;
                    }
                    core::hint::spin_loop();
                }
                crate::binfo!(
                    "wasm-ap",
                    "ran_on=core{} ok={} spawned={} (expect core2 ok=1)",
                    ran, ok, spawned,
                );
            } else {
                crate::binfo!("wasm-ap", "skipped (<3 cores or core2 not ComputeApp)");
            }
        }
    }

    // C2a boot-check: spawn `cwasm_ap_probe` onto core 2 (a ComputeApp core)
    // and wait for it to complete. The probe calls `run_echo_demo()` (the real
    // WASI path: shared engine + WASI Linker + argv + per-instance Store) ON
    // THAT CORE — proving the full run_cwasm WASI path works off the BSP.
    // This de-risks C2b: routing real exec'd .cwasm apps to ComputeApp cores.
    //
    // Skipped if fewer than 3 CPUs or core 2 is not a ComputeApp core.
    #[cfg(feature = "boot-checks")]
    {
        if crate::cpu::cpus_online() >= 3
            && crate::cpu::core_role(2) == crate::cpu::CoreRole::ComputeApp
        {
            // Retry until core 2 has published its SendSpawner.
            let mut spawned = false;
            for _ in 0..1_000_000u64 {
                if crate::executor::spawn_on(2, crate::executor::cwasm_ap_probe()).is_ok() {
                    spawned = true;
                    break;
                }
                core::hint::spin_loop();
            }
            // Wait for the probe to finish. run_cwasm (WASI Linker + Store +
            // instantiation + _start) is heavier than hello — give a generous budget.
            let mut code: i32 = i32::MIN;
            let mut ran: u32 = u32::MAX;
            for _ in 0..200_000_000u64 {
                let c = crate::executor::CWASM_AP_CODE
                    .load(core::sync::atomic::Ordering::SeqCst);
                if c != i32::MIN {
                    code = c;
                    ran  = crate::executor::CWASM_AP_RAN_ON
                        .load(core::sync::atomic::Ordering::SeqCst);
                    break;
                }
                core::hint::spin_loop();
            }
            crate::binfo!(
                "cwasm-ap",
                "ran_on=core{} code={} spawned={} (expect core2, echo exit code)",
                ran, code, spawned,
            );
        } else {
            crate::binfo!("cwasm-ap", "skipped (<3 cores or core2 not ComputeApp)");
        }
    }

    // Step 4 (pty-core) boot-check: routed-write gate. Spawn `pty_route_probe`
    // onto core 2 (a ComputeApp core). It calls `route_write_to_owner` for a
    // test pair — an OFF-OWNER write that must hop to the owner (BSP) over the
    // inbox bus and be processed there. Proves the app-core stdout path routes
    // to the owner instead of locking the pair cross-core. Gate:
    // `from=core2 routed_ok=true`.
    //
    // Skipped if fewer than 3 CPUs or core 2 is not a ComputeApp core.
    #[cfg(feature = "boot-checks")]
    {
        if crate::cpu::cpus_online() >= 2
            && crate::cpu::core_role(2) == crate::cpu::CoreRole::ComputeApp
        {
            let mut spawned = false;
            for _ in 0..1_000_000u64 {
                if crate::executor::spawn_on(2, crate::executor::pty_route_probe()).is_ok() {
                    spawned = true;
                    break;
                }
                core::hint::spin_loop();
            }
            // Wait for the probe to publish its result. The routed write hops
            // to the owner (BSP = core 0 = THIS core) over the inbox bus; the
            // owner-side op runs when core 0 drains its inbox. During this boot
            // phase the BSP is spinning here, NOT yet in its executor poll loop
            // (which is what drains the inbox at runtime), so we MUST drain it
            // by hand or the routed op never runs (the reply would never fire).
            let mut ran: u32 = u32::MAX;
            let mut ok: u32 = 2;
            for _ in 0..200_000_000u64 {
                crate::smp::inbox::drain_inbox(0);
                let o = crate::executor::PTY_ROUTE_OK
                    .load(core::sync::atomic::Ordering::SeqCst);
                if o != 2 {
                    ok = o;
                    ran = crate::executor::PTY_ROUTE_RAN_ON
                        .load(core::sync::atomic::Ordering::SeqCst);
                    break;
                }
                core::hint::spin_loop();
            }
            crate::binfo!(
                "pty-route",
                "from=core{} routed_ok={} spawned={}",
                ran, ok == 1, spawned,
            );
        } else {
            crate::binfo!("pty-route", "skipped (<3 cores or core2 not ComputeApp)");
        }
    }

    // C2d boot-check: parallelism gate. Proves that TWO `.cwasm` apps actually
    // EXECUTE on TWO DISTINCT ComputeApp cores AT THE SAME WALL TIME — the real
    // throughput win. Each probe now runs the CPU-heavy spin guest via the REAL
    // `run_cwasm(spin.cwasm)` path (WASI linker + instantiate + execute), so the
    // overlap measured here exercises wasmtime concurrency (custom-sync-primitives
    // + per-core TLS), NOT a bare compute loop.
    //
    // Protocol (≥ 4 CPUs, cores 2+3 both ComputeApp):
    //   1. Run ONE `parallel_probe(0, N)` on core 2 alone and time it → single_ms.
    //   2. Simultaneously spawn `parallel_probe(0, N)` on core 2 AND
    //      `parallel_probe(1, N)` on core 3; time the wall clock → concurrent_ms.
    //   3. concurrent_ms ≈ single_ms (not ≈ 2×single_ms) → overlap proven → PASS.
    //   4. Each probe also records `cpu_id()` → ran=[c0,c1] must be distinct.
    //
    // N is kept at 1 because each iteration is now a full ~300-800 ms
    // run_cwasm(spin.cwasm), not cheap arithmetic — one run is plenty measurable.
    //
    // Skipped on < 4 CPUs (core 3 does not exist) or if cores 2/3 are not
    // ComputeApp (unexpected role assignment).
    #[cfg(feature = "boot-checks")]
    {
        // cpus_online() = number of APs (excludes BSP). With -smp 4: 3 APs online.
        // Layout: core 0=BspIo, core 1=GuiCompositor, core 2+=ComputeApp.
        // ≥ 2 APs (cpus_online() ≥ 2) → core 2 exists.
        // ≥ 3 APs (cpus_online() ≥ 3) → core 3 exists.
        let has_core2_compute = crate::cpu::cpus_online() >= 2
            && crate::cpu::core_role(2) == crate::cpu::CoreRole::ComputeApp;
        let has_core3_compute = crate::cpu::cpus_online() >= 3
            && crate::cpu::core_role(3) == crate::cpu::CoreRole::ComputeApp;

        if has_core2_compute && has_core3_compute {
            const ITERS: u32 = 1; // 1 × run_cwasm(spin.cwasm) per probe (~300-800 ms each)

            // ── Phase 0: single baseline ─────────────────────────────────────
            // Reset done counter + run ONE probe on core 2, timed.
            crate::executor::PARALLEL_DONE
                .store(0, core::sync::atomic::Ordering::SeqCst);
            crate::executor::PARALLEL_RAN[0]
                .store(u32::MAX, core::sync::atomic::Ordering::SeqCst);
            crate::executor::PARALLEL_RAN[1]
                .store(u32::MAX, core::sync::atomic::Ordering::SeqCst);

            // Use 100 Hz timer ticks for timing (each tick = 10 ms; calibrated
            // early in the interrupt phase before this code runs).
            // tsc_per_ms() is 0 until late calibration — use ticks instead.

            // Spawn ONE probe (idx=0) on core 2 and time it.
            let t0 = crate::timer::ticks();
            for _ in 0..1_000_000u64 {
                if crate::executor::spawn_on(
                    2,
                    crate::executor::parallel_probe(0, ITERS),
                ).is_ok() { break; }
                core::hint::spin_loop();
            }
            // Wait for that one probe to finish (up to 60 s).
            for _ in 0..6_000_000_000u64 {
                if crate::executor::PARALLEL_DONE
                    .load(core::sync::atomic::Ordering::SeqCst) >= 1
                { break; }
                core::hint::spin_loop();
            }
            let t1 = crate::timer::ticks();
            // Convert 100 Hz ticks to ms (10 ms per tick).
            let single_ms = t1.saturating_sub(t0) * 10;

            // ── Phase 1: concurrent run ──────────────────────────────────────
            // Reset both probes' state and spawn BOTH simultaneously.
            crate::executor::PARALLEL_DONE
                .store(0, core::sync::atomic::Ordering::SeqCst);
            crate::executor::PARALLEL_RAN[0]
                .store(u32::MAX, core::sync::atomic::Ordering::SeqCst);
            crate::executor::PARALLEL_RAN[1]
                .store(u32::MAX, core::sync::atomic::Ordering::SeqCst);

            let t2 = crate::timer::ticks();
            // Spawn probe 0 on core 2.
            for _ in 0..1_000_000u64 {
                if crate::executor::spawn_on(
                    2,
                    crate::executor::parallel_probe(0, ITERS),
                ).is_ok() { break; }
                core::hint::spin_loop();
            }
            // Spawn probe 1 on core 3.
            for _ in 0..1_000_000u64 {
                if crate::executor::spawn_on(
                    3,
                    crate::executor::parallel_probe(1, ITERS),
                ).is_ok() { break; }
                core::hint::spin_loop();
            }
            // Wait for BOTH probes to finish (up to 120 s).
            for _ in 0..12_000_000_000u64 {
                if crate::executor::PARALLEL_DONE
                    .load(core::sync::atomic::Ordering::SeqCst) >= 2
                { break; }
                core::hint::spin_loop();
            }
            let t3 = crate::timer::ticks();
            let concurrent_ms = t3.saturating_sub(t2) * 10;

            let c0 = crate::executor::PARALLEL_RAN[0]
                .load(core::sync::atomic::Ordering::SeqCst);
            let c1 = crate::executor::PARALLEL_RAN[1]
                .load(core::sync::atomic::Ordering::SeqCst);

            // Overlap check: concurrent ≈ single (not ≈ 2×single).
            // We use a generous threshold: concurrent ≤ 1.6 × single → overlap.
            let overlap = concurrent_ms > 0
                && single_ms > 0
                && concurrent_ms <= (single_ms * 8 / 5); // ≤ 1.6×

            crate::binfo!(
                "parallel-exec",
                "ran=[{},{}] concurrent_ms={} single_ms={} overlap={}",
                c0, c1, concurrent_ms, single_ms, overlap,
            );
        } else {
            crate::binfo!("parallel-exec", "skipped (<4 cores or cores 2/3 not ComputeApp)");
        }
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
        // framebuffer. Returns the forced bg size packed (w<<32)|h. The shell
        // booting AS the bg desktop (+ launcher → wm.spawn) is verified visually.
        let spd = crate::wasm::wt::run_spd_demo();
        crate::binfo!("wm", "spd: hostfns ok bg={}x{}", spd >> 32, spd & 0xffff_ffff);
    }

    Ok(())
}
