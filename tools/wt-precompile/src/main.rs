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
    let (component_mode, in_path, out_path) = match args.len() {
        3 => (false, &args[1], &args[2]),
        4 if args[1] == "--component" => (true, &args[2], &args[3]),
        _ => {
            eprintln!("usage: wt-precompile [--component] <in.wasm> <out.cwasm>");
            std::process::exit(2);
        }
    };
    let wasm = fs::read(in_path).expect("read input wasm");
    let mut config = Config::new();
    config.target("x86_64-unknown-none").expect("set target");
    // Signals-based traps are OFF in the kernel runtime; match the dependent
    // memory tunables it expects. memory_reservation MUST equal the kernel's
    // engine_config (hashed into the .cwasm, exact-match check at deserialize):
    // 256 MiB VA per linear memory → kernel-side MmapMemory + demand paging
    // (frames on touch) instead of MallocMemory from the kernel heap.
    config.signals_based_traps(false);
    config.memory_init_cow(false);
    // Epoch watchdog (kernel compositor): hashed tunable, MUST match the
    // kernel's engine_config — flipping it invalidates every existing .cwasm
    // (see docs/api/README.md §".cwasm compatibility").
    config.epoch_interruption(true);
    config.memory_reservation(256 << 20);
    config.memory_guard_size(0);
    // Runtime-only (not hashed): keep 0 here, the kernel sets its own.
    config.memory_reservation_for_growth(0);
    config.memory_may_move(true);
    #[cfg(target_arch = "x86_64")]
    unsafe {
        config.x86_float_abi_ok(true);
        config.detect_host_feature(feature_policy);
        // Force SSE4.1+ in codegen so cranelift inlines ROUNDSS for f32.floor/
        // ceil/trunc/nearest instead of emitting a libcall. The no_std wasmtime
        // libcall path for those rounds is broken in ruos (returns the input),
        // which corrupted egui's glyph px_bounds → garbled text. ROUNDSS runs on
        // the CPU and is correct. The kernel runtime stays compatible because its
        // detect_host_feature already reports sse4.1 present.
        config.cranelift_flag_set("has_sse3", "true");
        config.cranelift_flag_set("has_ssse3", "true");
        config.cranelift_flag_set("has_sse41", "true");
        config.cranelift_flag_set("has_sse42", "true");
    }
    let engine = Engine::new(&config).expect("create engine");
    let cwasm = if component_mode {
        engine.precompile_component(&wasm).expect("precompile component")
    } else {
        engine.precompile_module(&wasm).expect("precompile module")
    };
    fs::write(out_path, &cwasm).expect("write output cwasm");
    eprintln!("wrote {} ({} bytes)", out_path, cwasm.len());
}
