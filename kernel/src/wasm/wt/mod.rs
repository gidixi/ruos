//! Wasmtime no_std AOT runtime (spike). Runs precompiled `.cwasm` modules at
//! near-native speed (no Cranelift on-device). See
//! docs/superpowers/plans/2026-06-04-wasmtime-nostd-spike.md

pub mod platform;
pub mod state;
pub mod mem;
pub mod wasi;
pub mod gfx;

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

/// Embedded real `cat` tool (user-bin/cat.wasm precompiled). Exercises the WASI
/// file path: path_open + fd_read + fd_seek + fd_filestat_get + fd_close.
#[cfg(feature = "boot-checks")]
static CAT_CWASM: &[u8] = include_bytes!("cat.cwasm");

/// Embedded GUI host-fn smoke (tools/wt-gfxtest/gfx.wat precompiled): calls
/// gfx_info then gfx_blit a 2×2 red square. Exercises the ruos_gfx Linker path.
#[cfg(feature = "boot-checks")]
static GFXTEST_CWASM: &[u8] = include_bytes!("gfxtest.cwasm");

/// Boot self-test: run the embedded gfx test; returns its exit code. The caller
/// inspects `crate::gfx::blit_count()` / `last_pixel()` (set during gfx_blit,
/// not cleared by the console restore) to confirm the host-fn path ran.
#[cfg(feature = "boot-checks")]
pub fn run_gfxtest_demo() -> i32 {
    run_cwasm(GFXTEST_CWASM, alloc::vec![b"gfxtest".to_vec()], None)
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

/// Shared engine (config is fixed; building it once avoids repeat cost).
pub fn engine() -> &'static wasmtime::Engine {
    static ENGINE: spin::Once<wasmtime::Engine> = spin::Once::new();
    ENGINE.call_once(|| wasmtime::Engine::new(&engine_config()).expect("wt engine"))
}

/// Load and run a precompiled WASI `.cwasm` command (`_start`) with `args`.
/// `pts`: Some(n) routes stdout/stderr to /dev/pts/n (bound terminal/SSH);
/// None fans out to CONSOLE (serial + framebuffer). Returns the guest exit code.
pub fn run_cwasm(cwasm: &[u8], args: Vec<Vec<u8>>, pts: Option<usize>) -> i32 {
    use wasmtime::{Module, Store, Linker};
    let engine = engine();
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
    let mut linker = Linker::new(engine);
    if let Err(e) = wasi::add_to_linker(&mut linker) {
        kprintln!("ruos: wt wasi link err: {}", e);
        return 126;
    }
    if let Err(e) = gfx::add_to_linker(&mut linker) {
        kprintln!("ruos: wt gfx link err: {}", e);
        return 126;
    }
    let instance = match linker.instantiate(&mut store, &module) {
        Ok(i) => i,
        Err(e) => { kprintln!("ruos: wt instantiate err: {:?}", e); return 126; }
    };
    if let Ok(start) = instance.get_typed_func::<(), ()>(&mut store, "_start") {
        let _ = start.call(&mut store, ()); // proc_exit traps; ignore
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
    // NB: no `target()` call — the kernel is compiled FOR x86_64-unknown-none, so
    // that IS the native target and must not be overridden (wasmtime rejects a
    // target that doesn't match the host). These tunables must match
    // tools/wt-precompile exactly so the AOT module's settings hash matches.
    config.signals_based_traps(false);
    config.memory_init_cow(false);
    config.memory_reservation(0);
    config.memory_guard_size(0);
    config.memory_reservation_for_growth(0);
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
