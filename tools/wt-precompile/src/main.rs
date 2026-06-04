//! Precompile a `.wasm` into a `.cwasm` compatible with the kernel's no_std
//! Wasmtime runtime. Usage: `wt-precompile <in.wasm> <out.cwasm>`.
//!
//! Compatibility requires the compile-time Config to match the kernel's runtime
//! Config EXACTLY. The subtle part is the x86 ISA: wasmtime otherwise infers the
//! BUILD HOST's native CPU features (avx/avx512/bmi/...) and bakes them into the
//! `.cwasm`, tying it to this machine. We instead pin a FIXED, deterministic
//! feature set via `detect_host_feature` (identical to the kernel), so the
//! module targets a portable baseline every x86_64 since ~2008 supports.

use std::{env, fs};
use wasmtime::{Config, Engine};

/// Fixed ISA policy shared with the kernel runtime (kernel/src/wasm/wt/mod.rs).
/// Returning fixed values (not a CPUID query) makes the compile and the runtime
/// agree deterministically.
fn feature_policy(feature: &str) -> Option<bool> {
    Some(matches!(feature, "sse3" | "ssse3" | "sse4.1" | "sse4.2"))
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: wt-precompile <in.wasm> <out.cwasm>");
        std::process::exit(2);
    }
    let wasm = fs::read(&args[1]).expect("read input wasm");
    let mut config = Config::new();
    config.target("x86_64-unknown-none").expect("set target");
    // Signals-based traps are OFF in the kernel runtime; match the dependent
    // memory tunables it expects (no CoW, no virtual reservation/guard pages).
    config.signals_based_traps(false);
    config.memory_init_cow(false);
    config.memory_reservation(0);
    config.memory_guard_size(0);
    config.memory_reservation_for_growth(0);
    // Allow linear memory to be a movable malloc()'d region so no host virtual
    // memory is required (the kernel has no mmap for guest memory).
    config.memory_may_move(true);
    #[cfg(target_arch = "x86_64")]
    unsafe {
        config.x86_float_abi_ok(true);
        config.detect_host_feature(feature_policy);
    }
    let engine = Engine::new(&config).expect("create engine");
    let cwasm = engine.precompile_module(&wasm).expect("precompile module");
    fs::write(&args[2], &cwasm).expect("write output cwasm");
    eprintln!("wrote {} ({} bytes)", &args[2], cwasm.len());
}
