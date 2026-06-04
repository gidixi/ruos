//! `ruos:gui` typed host functions for a GUI `.cwasm` (Wasmtime).
//!
//! This is the Canonical-ABI codec for the `ruos:gui` WIT world
//! (`wit/ruos-gui.wit`). The desktop guest (`ruos-backend`) generates its host
//! imports with wit-bindgen in **core-module mode** (NOT a component); the
//! imports lower via the Canonical ABI into named core-module imports under the
//! `"ruos:gui/gfx"` and `"ruos:gui/power"` modules, which we resolve here with
//! `func_wrap`. We decode/encode the Canonical ABI by hand via `wt::mem`, and
//! call the same `crate::gfx::*` / `crate::power::*` primitives the legacy
//! `ruos_gfx` linker (`wt/gfx.rs`) uses — so rendering is byte-identical.
//!
//! Flattened import signatures (captured from `wasm-tools print` of the guest,
//! wit-bindgen 0.57) — the `func_wrap` closures below MUST match these exactly:
//!   ruos:gui/gfx "get-info"     (func (param i32))                 ; retptr
//!   ruos:gui/gfx "blit"         (func (param i32 i32 i32 i32 i32 i32)) ; ptr len x y w h
//!   ruos:gui/gfx "poll-event"   (func (param i32))                 ; retptr (option<gfx-event>)
//!   ruos:gui/gfx "pending"      (func (result i32))
//!   ruos:gui/gfx "wall-seconds" (func (result f64))
//!   ruos:gui/gfx "debug-log"    (func (param i32 i32))             ; ptr len
//!   ruos:gui/power "poweroff"   (func)
//!   ruos:gui/power "reboot"     (func)

use wasmtime::{Caller, Linker};
use crate::wasm::wt::state::WtState;
use crate::wasm::wt::mem;
use crate::wasm::wt::gfx::wall_secs;

pub fn add_to_linker(linker: &mut Linker<WtState>) -> wasmtime::Result<()> {
    // gfx.get-info() -> gfx-info, lowered to (retptr). Writes the 16-byte record
    // {width,height,stride,format} as 4×u32 LE at `retptr` (same bytes as the
    // legacy gfx_info). Calling it marks the guest as a GUI app → GUI mode.
    linker.func_wrap("ruos:gui/gfx", "get-info",
        |mut caller: Caller<'_, WtState>, retptr: i32| {
            crate::gfx::enter();
            let g = crate::gfx::geom();
            let mut buf = [0u8; 16];
            buf[0..4].copy_from_slice(&g.width.to_le_bytes());
            buf[4..8].copy_from_slice(&g.height.to_le_bytes());
            buf[8..12].copy_from_slice(&g.stride.to_le_bytes());
            buf[12..16].copy_from_slice(&g.format.to_le_bytes());
            mem::write(&mut caller, retptr as u32, &buf);
        })?;

    // gfx.blit(pixels: list<u8>, x, y, w, h), lowered to (ptr len x y w h).
    linker.func_wrap("ruos:gui/gfx", "blit",
        |mut caller: Caller<'_, WtState>, ptr: i32, len: i32, x: i32, y: i32, w: i32, h: i32| {
            if let Some(bytes) = mem::read(&mut caller, ptr as u32, len as u32) {
                crate::gfx::blit(&bytes, x as u32, y as u32, w as u32, h as u32);
            }
        })?;

    // gfx.poll-event() -> option<gfx-event>, lowered to (retptr). The guest reads
    // the 20-byte return area as: discriminant i32 @0 (low byte: 0=none,1=some),
    // then the gfx-event record at offset 4 — kind@4, p0@8, p1@12, p2@16 (the
    // option payload offset is 4 = the record's alignment). We fold the PS/2
    // mouse first (the guest loops poll-event until none, so this matches the
    // legacy "fold then drain" semantics) and drain exactly ONE event.
    linker.func_wrap("ruos:gui/gfx", "poll-event",
        |mut caller: Caller<'_, WtState>, retptr: i32| {
            crate::gfx::fold_mouse();
            let mut buf = [0u8; 20];
            match crate::gfx::pop() {
                Some(e) => {
                    buf[0..4].copy_from_slice(&1u32.to_le_bytes());   // some
                    buf[4..8].copy_from_slice(&e.kind.to_le_bytes());
                    buf[8..12].copy_from_slice(&e.p0.to_le_bytes());
                    buf[12..16].copy_from_slice(&e.p1.to_le_bytes());
                    buf[16..20].copy_from_slice(&e.p2.to_le_bytes());
                }
                None => {
                    // discriminant 0 (none); payload bytes left zeroed.
                }
            }
            mem::write(&mut caller, retptr as u32, &buf);
        })?;

    // gfx.pending() -> u32: fold the PS/2 mouse then return the queued count.
    linker.func_wrap("ruos:gui/gfx", "pending",
        |_caller: Caller<'_, WtState>| -> i32 {
            crate::gfx::fold_mouse();
            crate::gfx::pending() as i32
        })?;

    // gfx.wall-seconds() -> f64: MONOTONIC seconds since local midnight. Reuses
    // the latched clock in wt/gfx.rs so behaviour is identical (no second latch).
    linker.func_wrap("ruos:gui/gfx", "wall-seconds",
        |_caller: Caller<'_, WtState>| -> f64 { wall_secs() })?;

    // gfx.debug-log(msg: string), lowered to (ptr len). Log the UTF-8 string to
    // the kernel serial (the PTY isn't drained while a sync GUI owns the executor).
    linker.func_wrap("ruos:gui/gfx", "debug-log",
        |mut caller: Caller<'_, WtState>, ptr: i32, len: i32| {
            if let Some(b) = mem::read(&mut caller, ptr as u32, len as u32) {
                if let Ok(s) = core::str::from_utf8(&b) {
                    crate::kprintln!("[gui] {}", s);
                }
            }
        })?;

    // power.poweroff() / power.reboot(): never return (the `-> ()` annotation
    // pins the closure's Wasm return type so never-type fallback stays `()`).
    linker.func_wrap("ruos:gui/power", "poweroff",
        |_caller: Caller<'_, WtState>| -> () { crate::power::poweroff() })?;
    linker.func_wrap("ruos:gui/power", "reboot",
        |_caller: Caller<'_, WtState>| -> () { crate::power::reboot() })?;

    Ok(())
}
