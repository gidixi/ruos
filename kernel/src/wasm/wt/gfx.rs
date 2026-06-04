//! `ruos_gfx` host functions for a GUI `.cwasm` (Wasmtime). Memory via wt::mem.
//! ABI mirrors ruos-desktop/gui-core (GfxInfo 4×u32, format RGBA8888=0; events
//! 16 bytes [kind,p0,p1,p2] — see crate::gfx::GfxEvt).

use wasmtime::{Caller, Linker};
use crate::wasm::wt::state::WtState;
use crate::wasm::wt::mem;

pub fn add_to_linker(linker: &mut Linker<WtState>) -> wasmtime::Result<()> {
    // gfx_info(out_ptr) -> 0; writes GfxInfo{w,h,stride,format} (4×u32 LE).
    // Calling this marks the guest as a GUI app → enter GUI mode (console quiets).
    linker.func_wrap("ruos_gfx", "gfx_info",
        |mut caller: Caller<'_, WtState>, out: i32| -> i32 {
            crate::gfx::enter();
            let g = crate::gfx::geom();
            let mut buf = [0u8; 16];
            buf[0..4].copy_from_slice(&g.width.to_le_bytes());
            buf[4..8].copy_from_slice(&g.height.to_le_bytes());
            buf[8..12].copy_from_slice(&g.stride.to_le_bytes());
            buf[12..16].copy_from_slice(&g.format.to_le_bytes());
            if mem::write(&mut caller, out as u32, &buf) { 0 } else { 28 }
        })?;

    // gfx_blit(buf_ptr, buf_len, x, y, w, h) -> 0
    linker.func_wrap("ruos_gfx", "gfx_blit",
        |mut caller: Caller<'_, WtState>, ptr: i32, len: i32, x: i32, y: i32, w: i32, h: i32| -> i32 {
            let bytes = match mem::read(&mut caller, ptr as u32, len as u32) {
                Some(b) => b, None => return 28,
            };
            crate::gfx::blit(&bytes, x as u32, y as u32, w as u32, h as u32);
            0
        })?;

    // gfx_poll_event(out_ptr, max, timeout_ms) -> count of events written.
    // Each event is 16 bytes. (Poll-based: returns immediately; timeout unused
    // until epoch-yield blocking lands.)
    linker.func_wrap("ruos_gfx", "gfx_poll_event",
        |mut caller: Caller<'_, WtState>, out: i32, max: i32, _timeout_ms: i32| -> i32 {
            crate::gfx::fold_mouse();
            let mut n = 0i32;
            while n < max {
                match crate::gfx::pop() {
                    Some(e) => {
                        let mut buf = [0u8; 16];
                        buf[0..4].copy_from_slice(&e.kind.to_le_bytes());
                        buf[4..8].copy_from_slice(&e.p0.to_le_bytes());
                        buf[8..12].copy_from_slice(&e.p1.to_le_bytes());
                        buf[12..16].copy_from_slice(&e.p2.to_le_bytes());
                        let off = out as u32 + (n as u32) * 16;
                        if !mem::write(&mut caller, off, &buf) { break; }
                        n += 1;
                    }
                    None => break,
                }
            }
            n
        })?;

    // gfx_wall_secs() -> f64: seconds (with fraction) since local midnight.
    // For egui RawInput.time + the desktop clock (Platform::wall_clock_secs).
    linker.func_wrap("ruos_gfx", "gfx_wall_secs",
        |_caller: Caller<'_, WtState>| -> f64 {
            let t = crate::rtc::now();
            let frac = (crate::timer::ticks() % 100) as f64 / 100.0;
            (t.hour as f64) * 3600.0 + (t.minute as f64) * 60.0 + (t.second as f64) + frac
        })?;

    Ok(())
}
