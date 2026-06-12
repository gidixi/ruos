//! Wasmtime no_std AOT runtime (spike). Runs precompiled `.cwasm` modules at
//! near-native speed (no Cranelift on-device). See
//! docs/superpowers/plans/2026-06-04-wasmtime-nostd-spike.md

pub mod platform;
pub mod demand;
pub mod state;
pub mod mem;
pub mod wasi;
pub mod gfx;
pub mod gui;
pub mod component;
pub mod wm;
pub mod term;
pub mod compose;
pub mod sys;
pub mod net;
pub mod threads;

use crate::kprintln;
use alloc::vec::Vec;
use crate::wasm::wt::state::WtState;

/// Embedded AOT demo module: `tools/wt-hello/hello.wat` compiled to a target
/// `.cwasm` by the host build step (`wasmtime compile --target
/// x86_64-unknown-none`) and copied next to this file. Exercised by the boot
/// self-test to prove the no_std AOT runtime end-to-end.
#[cfg(feature = "boot-checks")]
static HELLO_CWASM: &[u8] = include_bytes!("hello.cwasm");

/// Embedded real `wasm32-wasip1` tool (`user-bin/echo.wasm`) precompiled to a
/// matching `.cwasm`. Exercises the WASI Preview 1 path with a genuine std
/// binary (argv + fd_write), not just the hand-rolled `ruos.print` demo.
#[cfg(feature = "boot-checks")]
static ECHO_CWASM: &[u8] = include_bytes!("echo.cwasm");

/// Boot self-test: run the embedded `hello.cwasm`. Returns true iff its `run`
/// export called `ruos.print(42)`.
#[cfg(feature = "boot-checks")]
pub fn run_hello_demo() -> bool {
    run_hello(HELLO_CWASM)
}

/// Boot self-test: run the embedded `echo.cwasm` with argv ["echo", "WT-ECHO-OK"].
/// Its output reaches the serial log via stdout → CONSOLE; the caller greps it.
#[cfg(feature = "boot-checks")]
pub fn run_echo_demo() -> i32 {
    run_cwasm(ECHO_CWASM, alloc::vec![b"echo".to_vec(), b"WT-ECHO-OK".to_vec()], None)
}

/// Embedded CPU-heavy spin guest (`tools/wt-spin/spin.wat` precompiled). Busy-
/// loops ~2e9 iterations (~300-800 ms on QEMU) then exits 0. Used by the C2d
/// parallel-exec gate to put real, measurable wasm load on two cores at once.
#[cfg(feature = "boot-checks")]
static SPIN_CWASM: &[u8] = include_bytes!("spin.cwasm");

/// Boot-check: run the CPU-heavy spin guest via the REAL run_cwasm path
/// (WASI linker + instantiate + execute). Returns the guest exit code (0).
#[cfg(feature = "boot-checks")]
pub fn run_spin_demo() -> i32 {
    run_cwasm(SPIN_CWASM, alloc::vec::Vec::new(), None)
}

/// Gate 1 MT Fase 2 (tools/wt-threads-gate/gate1.wat precompilato): modulo CORE
/// con memoria SHARED importata che fa RMW atomici e rilegge il valore.
#[cfg(feature = "boot-checks")]
static THREADS_GATE1_CWASM: &[u8] = include_bytes!("threads_gate1.cwasm");

/// Gate 1 MT Fase 2: SharedMemory + atomics nativi nell'engine no_std (fork
/// third_party/wasmtime45, feature `threads`). Crea la SharedMemory host-side
/// dal MemoryType importato dal guest, la definisce nel linker come
/// `env.memory`, esegue due `i32.atomic.rmw.add` (lock-prefixed su x86 via
/// AOT) + un `i32.atomic.load`. true sse run() == 42. Niente atomic.wait:
/// gli stub futex temporanei di platform.rs non vengono toccati.
#[cfg(feature = "boot-checks")]
pub fn run_threads_gate1() -> bool {
    let engine = engine();
    // SAFETY: cwasm prodotto da tools/wt-precompile per questa exact config.
    let module = match unsafe { wasmtime::Module::deserialize(engine, THREADS_GATE1_CWASM) } {
        Ok(m) => m,
        Err(e) => { kprintln!("ruos: gate1 deserialize: {:?}", e); return false; }
    };
    let mem_ty = match module.imports().find_map(|i| i.ty().memory().cloned()) {
        Some(t) => t,
        None => { kprintln!("ruos: gate1: no memory import"); return false; }
    };
    let shared = match wasmtime::SharedMemory::new(engine, mem_ty) {
        Ok(s) => s,
        Err(e) => { kprintln!("ruos: gate1 SharedMemory: {:?}", e); return false; }
    };
    let mut store = wasmtime::Store::new(engine, ());
    store.set_epoch_deadline(NO_DEADLINE_TICKS); // boot-check, not watched
    let mut linker: wasmtime::Linker<()> = wasmtime::Linker::new(engine);
    if let Err(e) = linker.define(&store, "env", "memory", shared.clone()) {
        kprintln!("ruos: gate1 define: {:?}", e);
        return false;
    }
    let inst = match linker.instantiate(&mut store, &module) {
        Ok(i) => i,
        Err(e) => { kprintln!("ruos: gate1 instantiate: {:?}", e); return false; }
    };
    let run = match inst.get_typed_func::<(), i32>(&mut store, "run") {
        Ok(f) => f,
        Err(e) => { kprintln!("ruos: gate1 no run export: {}", e); return false; }
    };
    // DF=0 prima di entrare nel guest (convenzione di ogni call site wt).
    #[cfg(target_arch = "x86_64")]
    unsafe { core::arch::asm!("cld", options(nostack)); }
    match run.call(&mut store, ()) {
        Ok(42) => true,
        other => { kprintln!("ruos: gate1 run = {:?} (want Ok(42))", other); false }
    }
}

/// Embedded real `cat` tool (user-bin/cat.wasm precompiled). Exercises the WASI
/// file path: path_open + fd_read + fd_seek + fd_filestat_get + fd_close.
#[cfg(feature = "boot-checks")]
static CAT_CWASM: &[u8] = include_bytes!("cat.cwasm");

#[cfg(feature = "boot-checks")]
static BRINGUP_CWASM: &[u8] = include_bytes!("bringup.cwasm");

/// Boot self-test: SP3 window-manager pure-logic selftest (decoration geometry +
/// hit-test + z-order + drag math, NO wasm instances). Returns a 5-bit flag word
/// (0b11111 == all sub-checks pass).
#[cfg(feature = "boot-checks")]
pub fn run_wm_logic_selftest() -> u32 {
    crate::wasm::wt::wm::wm_logic_selftest()
}

/// Boot self-test: run the embedded bring-up component; its `run` calls
/// system.log("WT-COMPONENT-OK") on the host. Returns the guest run() code.
#[cfg(feature = "boot-checks")]
pub fn run_bringup_demo() -> i32 {
    crate::wasm::wt::component::run_component(BRINGUP_CWASM)
}

/// Boot self-test: the SP5 launcher registry has N entries and all deserialise.
/// Returns (entry_count, modules_ok).
#[cfg(feature = "boot-checks")]
pub fn run_registry_demo() -> (u32, u32) {
    crate::wasm::wt::wm::registry_self_test()
}

/// Boot self-test: spawn the self-closing app, run the loop, confirm teardown +
/// window-id recycle. Returns (spawns, peak_live, final_live).
#[cfg(feature = "boot-checks")]
pub fn run_lifecycle_demo() -> (u32, u32, u32) {
    crate::wasm::wt::wm::lifecycle_self_test()
}

/// Boot self-test (egui SP-B): spawn the egui CSD demo as a compositor window and
/// drive one frame; returns the committed surface length (614400 on success =
/// 480×320×4 — the egui guest instantiated against the unified WASI+wm
/// `Linker<AppState>`, ran one egui ctx.run + tessellate + raster, and committed).
#[cfg(feature = "boot-checks")]
pub fn run_egui_demo_demo() -> usize {
    crate::wasm::wt::wm::egui_demo_self_test()
}

/// Boot self-test (epoch watchdog): a deliberately-spinning window must be
/// trapped + reaped while a healthy reactor keeps ticking. Returns
/// (spinner_reaped, healthy_tick).
#[cfg(feature = "boot-checks")]
pub fn run_watchdog_demo() -> (bool, u32) {
    crate::wasm::wt::wm::watchdog_self_test()
}

/// Boot self-test (egui SP-C): exercise the `wm.spawn` deferred-spawn mechanism +
/// the `wm.set_background` full-screen-bg mechanism headlessly (embedded module,
/// since `/bin` isn't mounted this early). Returns a 2-bit flag word (`0b11` ==
/// spawn grew the window list to 2 AND a bg window was forced full-screen). The
/// VFS `wm.spawn` (loading `/bin/egui-demo.cwasm`) is covered visually.
#[cfg(feature = "boot-checks")]
pub fn run_spc_demo() -> u32 {
    crate::wasm::wt::wm::spc_self_test()
}

/// Boot self-test (egui SP-D): prove the desktop-shell boot wiring without the VFS —
/// the `wm.poweroff`/`wm.surface_size` host fns register (the empty compositor builds)
/// AND the `wm.set_background` full-screen mechanism the shell uses still works.
/// Returns the forced bg size packed `(w<<32)|h` (0 == failed). The shell-as-bg
/// desktop + launcher→`wm.spawn` is verified visually.
#[cfg(feature = "boot-checks")]
pub fn run_spd_demo() -> i64 {
    crate::wasm::wt::wm::spd_self_test()
}

/// Boot self-test: seed a tmpfs file with a known marker, then `cat` it. The
/// marker reaches the serial log via cat's stdout; the caller greps it.
#[cfg(feature = "boot-checks")]
pub fn run_cat_demo() -> i32 {
    let _ = crate::vfs::block_on(async {
        let fd = crate::vfs::open(
            "/wt-cat-test.txt",
            crate::vfs::OpenFlags::CREATE | crate::vfs::OpenFlags::WRITE,
        ).await?;
        crate::vfs::write(fd, b"CAT-OK-MARKER\n").await?;
        crate::vfs::close(fd).await?;
        Ok::<(), crate::vfs::VfsError>(())
    });
    run_cwasm(CAT_CWASM, alloc::vec![b"cat".to_vec(), b"/wt-cat-test.txt".to_vec()], None)
}

// --- Epoch watchdog (spec 2026-06-11-wt-epoch-interruption-design) ----------
//
// The engine has `epoch_interruption(true)`: AOT guest code checks an engine
// atomic at function entries + loop backedges and TRAPS (`Trap::Interrupt`)
// once the per-Store deadline expires. The BSP timer IRQ bumps the epoch at
// 100 Hz → 1 tick = 10 ms. There is NO resume (wasmtime `async` is off):
// the watchdog turns "desktop frozen forever" into "guilty window killed".
//
// CRITICAL: with epoch_interruption on, a Store that never calls
// `set_epoch_deadline` traps IMMEDIATELY (default deadline 0). Every
// `Store::new` site must arm one of these:

/// Steady-state `frame()` budget (~1 s): generous enough for a legitimate
/// heavy relayout (GATE: 6400 nodes = 28 ms native; 50k ≈ 220 ms), still
/// converts a runaway guest into a ~1 s blip. NB: QEMU TCG inflates guest
/// time 10-30× — values are tuned to avoid false kills there too.
pub const FRAME_DEADLINE_TICKS: u64 = 300;
/// First `frame()` of a window (~3 s): parse/style/layout + font atlas + egui
/// init all land here (viewer ≈ 30-60 ms native, seconds under QEMU TCG).
pub const FIRST_FRAME_DEADLINE_TICKS: u64 = 300;
/// `_initialize` (std static ctors), one-shot per window (~3 s).
pub const INIT_DEADLINE_TICKS: u64 = 300;
/// Throwaway `manifest()` probe (~1 s): const data in the guest data segment,
/// near-instant; protects the ~1 Hz catalog scan from a hostile .cwasm.
pub const PROBE_DEADLINE_TICKS: u64 = 100;
/// Effectively-infinite deadline for non-compositor stores (CLI tools, boot
/// checks, component bring-up): they may legitimately run for a long time.
pub const NO_DEADLINE_TICKS: u64 = u64::MAX / 2;

static ENGINE: spin::Once<wasmtime::Engine> = spin::Once::new();

/// Shared engine (config is fixed; building it once avoids repeat cost).
pub fn engine() -> &'static wasmtime::Engine {
    ENGINE.call_once(|| wasmtime::Engine::new(&engine_config()).expect("wt engine"))
}

/// Bump the wasm epoch — called from the BSP timer IRQ (100 Hz). IRQ-safe:
/// `increment_epoch` is one Relaxed `fetch_add`, and `Once::get` (never
/// `call_once`) guarantees we never CONSTRUCT the engine in IRQ context.
pub fn epoch_tick() {
    if let Some(e) = ENGINE.get() {
        e.increment_epoch();
    }
}

/// Load and run a precompiled WASI `.cwasm` command (`_start`) with `args`.
/// `pts`: Some(n) routes stdout/stderr to /dev/pts/n (bound terminal/SSH);
/// None fans out to CONSOLE (serial + framebuffer). Returns the guest exit code.
///
/// No global lock: concurrent `run_cwasm` calls across ComputeApp cores are made
/// safe by wasmtime's `custom-sync-primitives` feature (the cross-core spinlock
/// shims in `platform.rs` replace the no_std default that panics on contention)
/// plus per-core TLS (`platform.rs` `TLS[cpu_id()]`). wasmtime's internal locks
/// are fine-grained (brief type/module-registry inserts, not held across guest
/// execution), so multiple `.cwasm` apps run truly in parallel on separate cores.
pub fn run_cwasm(cwasm: &[u8], args: Vec<Vec<u8>>, pts: Option<usize>) -> i32 {
    use wasmtime::{Module, Store, Linker};
    let engine = engine();
    // Component artifacts (e.g. rtop.cwasm, `ruos:tui/tui-app` world) take the
    // component runner: dynamic-link against the shared /bin/tui.cwasm
    // provider. Core-module artifacts continue below on the WASI path.
    if matches!(wasmtime::Engine::detect_precompiled(cwasm), Some(wasmtime::Precompiled::Component)) {
        let tui = match crate::vfs::block_on(crate::wasm::read_all("/bin/tui.cwasm")) {
            Ok(b) => b,
            Err(_) => { kprintln!("ruos: /bin/tui.cwasm missing (tui provider)"); return 127; }
        };
        let once = args.iter().any(|a| a.as_slice() == b"--once");
        return component::run_tui_component(cwasm, &tui, once, pts);
    }
    // SAFETY: cwasm produced by tools/wt-precompile for this exact config.
    let module = match unsafe { Module::deserialize(engine, cwasm) } {
        Ok(m) => m,
        Err(e) => { kprintln!("ruos: wt deserialize err: {:?}", e); return 126; }
    };
    let mut state = WtState::new(args);
    // Bind stdout/stderr to the caller's PTY slave, if any.
    if let Some(n) = pts {
        let path = alloc::format!("/dev/pts/{}", n);
        if let Ok(fd) = crate::vfs::block_on(crate::vfs::open(&path, crate::vfs::OpenFlags::WRITE)) {
            state.stdout_pty = Some(fd);
        }
    }
    let mut store = Store::new(engine, state);
    // CLI tools may run as long as they like — the watchdog is for the
    // compositor only. Without this the default deadline (0) traps at once.
    store.set_epoch_deadline(NO_DEADLINE_TICKS);
    let mut linker = Linker::new(engine);
    if let Err(e) = wasi::add_to_linker(&mut linker) {
        kprintln!("ruos: wt wasi link err: {}", e);
        return 126;
    }
    if let Err(e) = gfx::add_to_linker(&mut linker) {
        kprintln!("ruos: wt gfx link err: {}", e);
        return 126;
    }
    if let Err(e) = gui::add_to_linker(&mut linker) {
        kprintln!("ruos: wt gui link err: {}", e);
        return 126;
    }
    let instance = match linker.instantiate(&mut store, &module) {
        Ok(i) => i,
        Err(e) => { kprintln!("ruos: wt instantiate err: {:?}", e); return 126; }
    };
    // Ensure the Direction Flag is clear: the SysV ABI requires DF=0 on entry,
    // and cranelift/Rust code uses `rep movs` (memcpy/memmove) which run BACKWARD
    // if DF=1, silently corrupting copied data (e.g. egui's font atlas) → garbled
    // glyphs. A bare-metal kernel must guarantee DF=0; firmware may leave it set.
    #[cfg(target_arch = "x86_64")]
    unsafe { core::arch::asm!("cld", options(nostack)); }
    match instance.get_typed_func::<(), ()>(&mut store, "_start") {
        Ok(start) => {
            if let Err(e) = start.call(&mut store, ()) {
                // proc_exit traps to unwind; only log unexpected traps.
                if store.data().exit.is_none() {
                    kprintln!("ruos: wt _start trap: {:?}", e);
                }
            }
        }
        Err(e) => kprintln!("ruos: wt no _start: {}", e),
    }
    if let Some(fd) = store.data().stdout_pty {
        let _ = crate::vfs::block_on(crate::vfs::close(fd));
    }
    // If the guest was a GUI app (called gfx_info), restore the text console.
    crate::gfx::leave();
    store.data().exit.unwrap_or(0)
}

/// Run a precompiled `.cwasm` whose `run` export calls imported `ruos.print`.
/// Returns true iff the guest invoked `print(42)`. Spike-grade error handling
/// (any failure → false); the point is to prove the AOT runtime works at all.
/// Build the runtime Config. Must match the AOT compile settings produced by
/// `tools/wt-precompile` (recipe from wasmtime examples/min-platform): on
/// x86_64 the `-unknown-none` target uses float registers, so we allow the
/// float ABI and supply a CPU-feature detector (no_std can't auto-detect) for
/// the sse3/sse4/fma features the cwasm was compiled with.
fn engine_config() -> wasmtime::Config {
    let mut config = wasmtime::Config::new();
    // These tunables must match tools/wt-precompile exactly so the AOT module's
    // settings hash matches.
    config.signals_based_traps(false);
    config.memory_init_cow(false);
    // Watchdog sui guest del compositor (hashato nel .cwasm! — cambiarlo
    // invalida ogni .cwasm esistente, vedi docs/api/README.md §compatibilità
    // e spec 2026-06-11-wt-epoch-interruption-design).
    config.epoch_interruption(true);
    config.wasm_threads(true); // feature bit nel .cwasm (check di SOTTOINSIEME a deserialize: .cwasm vecchi senza THREADS restano caricabili; spegnerlo qui rifiuta i .cwasm nuovi — vedi CHANGELOG/486)
    // Gate runtime-only (NON hashato nel .cwasm) per la CREAZIONE di
    // SharedMemory host-side: senza, `SharedMemory::new` fallisce con
    // "shared memory support is disabled for this engine" (gate 1 MT Fase 2).
    config.shared_memory(true);
    // memory_reservation > 0 forza il path MmapMemory (wasmtime memory.rs:
    // signals||guard||reservation||cow): la linear memory passa da
    // wasmtime_mmap_new → demand paging → FRAME allocator (RAM−heap), non più
    // MallocMemory dallo heap talc (48 MiB contigui per app → OOM alla 2ª
    // finestra). 256 MiB è solo VA riservata nella finestra WT (demand-paged:
    // i frame si pagano al touch); i grow entro la reservation sono in-place.
    config.memory_reservation(256 << 20);
    config.memory_guard_size(0);
    // Non hashata nel .cwasm (runtime-only): headroom per i move oltre la
    // reservation (new+copy+unmap), ammortizza i grow giganti.
    config.memory_reservation_for_growth(64 << 20);
    config.memory_may_move(true);
    #[cfg(target_arch = "x86_64")]
    unsafe {
        config.x86_float_abi_ok(true);
        config.detect_host_feature(|feature| {
            Some(matches!(feature, "sse3" | "ssse3" | "sse4.1" | "sse4.2"))
        });
    }
    config
}

pub fn run_hello(cwasm: &[u8]) -> bool {
    use wasmtime::{Engine, Module, Store, Linker};

    let engine = match Engine::new(&engine_config()) {
        Ok(e) => e,
        Err(e) => { kprintln!("ruos: wt engine err: {}", e); return false; }
    };

    // SAFETY: `cwasm` must have been produced by the matching wasmtime version
    // for this target. (Spike: caller controls the bytes.)
    let module = match unsafe { Module::deserialize(&engine, cwasm) } {
        Ok(m) => m,
        Err(e) => { kprintln!("ruos: wt deserialize err: {:?}", e); return false; }
    };

    // Store data = "did we see print(42)?" flag.
    let mut store = Store::new(&engine, false);
    store.set_epoch_deadline(NO_DEADLINE_TICKS); // boot-check, not watched
    let mut linker = Linker::new(&engine);
    let _ = linker.func_wrap(
        "ruos",
        "print",
        |mut caller: wasmtime::Caller<'_, bool>, v: i32| {
            kprintln!("ruos: wt hello print={}", v);
            if v == 42 {
                *caller.data_mut() = true;
            }
        },
    );

    let instance = match linker.instantiate(&mut store, &module) {
        Ok(i) => i,
        Err(e) => { kprintln!("ruos: wt instantiate err: {}", e); return false; }
    };
    let run = match instance.get_typed_func::<(), ()>(&mut store, "run") {
        Ok(f) => f,
        Err(e) => { kprintln!("ruos: wt get_func err: {}", e); return false; }
    };
    if let Err(e) = run.call(&mut store, ()) {
        kprintln!("ruos: wt call err: {}", e);
        return false;
    }
    *store.data()
}
