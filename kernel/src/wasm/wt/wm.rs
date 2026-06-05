//! Window-manager / compositor host module (`wm`) + reactor driver. Holds N
//! persistent wasm instances; calls their exported `frame()` round-robin; reads
//! each committed surface into the per-store WmState.
//!
//! The `wm` imports are raw `extern "C"` (not WIT) to keep this spike focused on
//! the concurrency mechanism: a PERSISTENT instance whose `frame()` export is
//! called repeatedly. WIT-ification comes when building the real compositor.

use alloc::vec::Vec;
use wasmtime::{Caller, Extern, Linker, Memory, Module, Store};
use crate::wasm::wt::engine;

/// Per-instance store data: window id + last committed surface.
pub struct WmState {
    pub id: u32,
    pub win_w: u32,
    pub win_h: u32,
    pub pixels: Vec<u8>,
    pub tick: u32,
}

/// Read `len` bytes from this guest's exported linear memory at `ptr`. None if
/// the export is missing or the range is out of bounds. (Mirrors
/// `crate::wasm::wt::mem::read`, which is typed to `WtState` and so cannot be
/// reused for `WmState`.)
fn read_guest(caller: &mut Caller<'_, WmState>, ptr: u32, len: u32) -> Option<Vec<u8>> {
    let mem = match caller.get_export("memory") {
        Some(Extern::Memory(m)) => m,
        _ => return None,
    };
    let mem: Memory = mem;
    let mut out = alloc::vec![0u8; len as usize];
    mem.read(caller, ptr as usize, &mut out).ok()?;
    Some(out)
}

pub fn add_to_linker(linker: &mut Linker<WmState>) -> wasmtime::Result<()> {
    // wm.commit(ptr, len, w, h): copy the guest's surface into WmState.pixels.
    linker.func_wrap("wm", "commit",
        |mut caller: Caller<'_, WmState>, ptr: i32, len: i32, w: i32, h: i32| {
            if let Some(b) = read_guest(&mut caller, ptr as u32, len as u32) {
                let s = caller.data_mut();
                s.pixels = b;
                s.win_w = w as u32;
                s.win_h = h as u32;
            }
        })?;
    // wm.app_id() -> u32: this instance's window id. (Import name is `app_id`
    // with an underscore — Rust `#[link]` preserves the symbol verbatim; verified
    // via `wasm-tools print`.)
    linker.func_wrap("wm", "app_id",
        |caller: Caller<'_, WmState>| -> i32 { caller.data().id as i32 })?;
    // wm.tick(): bump the call counter (spike instrumentation).
    linker.func_wrap("wm", "tick",
        |mut caller: Caller<'_, WmState>| { caller.data_mut().tick += 1; })?;
    Ok(())
}

/// SPIKE: instantiate ONE reactor instance, call `frame()` 5× on it, return
/// `(tick, first_pixel_byte0, pixels_len)`. Proves a persistent instance +
/// repeated export call AND that the committed surface buffer arrives intact.
pub fn run_reactor_spike(cwasm: &[u8]) -> (u32, u8, usize) {
    let engine = engine();
    // SAFETY: produced by wt-precompile for this exact engine Config.
    let module = match unsafe { Module::deserialize(engine, cwasm) } {
        Ok(m) => m,
        Err(_) => return (0, 0, 0),
    };
    let mut store = Store::new(engine, WmState { id: 0, win_w: 0, win_h: 0, pixels: Vec::new(), tick: 0 });
    let mut linker: Linker<WmState> = Linker::new(engine);
    if add_to_linker(&mut linker).is_err() { return (0, 0, 0); }
    // SysV ABI requires DF=0; cranelift/Rust code uses `rep movs` which run
    // BACKWARD if DF=1, silently corrupting copied data.
    #[cfg(target_arch = "x86_64")]
    unsafe { core::arch::asm!("cld", options(nostack)); }
    let instance = match linker.instantiate(&mut store, &module) {
        Ok(i) => i,
        Err(_) => return (0, 0, 0),
    };
    let frame = match instance.get_typed_func::<(), ()>(&mut store, "frame") {
        Ok(f) => f,
        Err(_) => return (0, 0, 0),
    };
    for _ in 0..5 {
        if frame.call(&mut store, ()).is_err() { break; }
    }
    (
        store.data().tick,
        store.data().pixels.first().copied().unwrap_or(0),
        store.data().pixels.len(),
    )
}

/// Visual GATE: 2 reactor instances, side-by-side, both updating. Owns the CPU
/// (like the single-GUI path today). Never returns.
///
/// Keeps two persistent `(Store<WmState>, Instance)` alive, calls each one's
/// exported `frame()` round-robin per loop, then blits each store's last
/// committed surface to its window rect (left half / right half of the screen).
/// Both windows visibly cycle colour (different per-id offset) — proving N
/// persistent instances + round-robin frame() + per-app surface + compositing.
pub fn run_compositor_gate(cwasm: &[u8]) -> ! {
    crate::kprintln!("[wm] compositor gate: 2 reactor windows");
    crate::gfx::enter();
    let engine = engine();
    // SAFETY: produced by wt-precompile for this exact engine Config.
    let module = unsafe { Module::deserialize(engine, cwasm) }.expect("reactor module");
    let mut linker: Linker<WmState> = Linker::new(engine);
    add_to_linker(&mut linker).expect("wm linker");
    // SysV ABI requires DF=0; cranelift/Rust code uses `rep movs` which run
    // BACKWARD if DF=1, silently corrupting copied data.
    #[cfg(target_arch = "x86_64")]
    unsafe { core::arch::asm!("cld", options(nostack)); }

    // Two windows: left and right halves of the screen (rect origins).
    let g = crate::gfx::geom();
    let origins = [(0u32, 0u32), (g.width / 2, 0u32)];
    let mut wins: Vec<(Store<WmState>, wasmtime::Instance, (u32, u32))> = Vec::new();
    for (id, &origin) in origins.iter().enumerate() {
        let mut store = Store::new(
            engine,
            WmState { id: id as u32, win_w: 0, win_h: 0, pixels: Vec::new(), tick: 0 },
        );
        let inst = linker.instantiate(&mut store, &module).expect("instantiate");
        wins.push((store, inst, origin));
    }
    loop {
        for (store, inst, origin) in wins.iter_mut() {
            if let Ok(frame) = inst.get_typed_func::<(), ()>(&mut *store, "frame") {
                let _ = frame.call(&mut *store, ());
            }
            let s = store.data();
            if !s.pixels.is_empty() {
                crate::gfx::blit(&s.pixels, origin.0, origin.1, s.win_w, s.win_h);
            }
        }
        // Crude pacing so the colour cycle is visible.
        for _ in 0..2_000_000u32 { core::hint::spin_loop(); }
    }
}
