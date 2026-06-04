//! `ruos_gfx` host functions for a GUI `.cwasm` (Wasmtime). Memory via wt::mem.
//! ABI mirrors ruos-desktop/gui-core (GfxInfo 4×u32, format RGBA8888=0; events
//! 16 bytes [kind,p0,p1,p2] — see crate::gfx::GfxEvt).

use core::sync::atomic::{AtomicI64, Ordering};
use wasmtime::{Caller, Linker};
use crate::wasm::wt::state::WtState;
use crate::wasm::wt::mem;

/// Monotonic wall-clock seconds for the GUI. egui's `RawInput.time` MUST be
/// monotonic. The previous formula — `rtc_second + (ticks()%100)/100` — mixed
/// two UNSYNCHRONIZED clocks (the CMOS RTC and the LAPIC tick counter): their
/// phases differ, so the tick fraction wrapped back to 0 before the RTC second
/// rolled and the value jumped backward ~1×/sec. egui then ran window/menu
/// open-animations backward then forward (windows appeared to close and reopen)
/// and stuttered. Fix: latch a wall offset ONCE from the RTC, then advance
/// purely by the monotonic uptime tick (10 ms resolution) — never goes backward
/// (egui-safe). The HH:MM clock widget stays correct (it wraps via rem_euclid);
/// tick-vs-RTC drift over a session is sub-second, irrelevant for minute display.
static WALL_OFFSET_CS: AtomicI64 = AtomicI64::new(i64::MIN); // centiseconds; MIN = uninit

pub fn wall_secs() -> f64 {
    let up_cs = crate::timer::ticks() as i64; // 100 Hz tick = 1 centisecond of uptime
    let mut off = WALL_OFFSET_CS.load(Ordering::Relaxed);
    if off == i64::MIN {
        let t = crate::rtc::now();
        let wall_cs =
            ((t.hour as i64) * 3600 + (t.minute as i64) * 60 + (t.second as i64)) * 100;
        off = wall_cs - up_cs;
        // First caller wins; any racing caller computes ~the same offset.
        let _ = WALL_OFFSET_CS.compare_exchange(
            i64::MIN, off, Ordering::Relaxed, Ordering::Relaxed,
        );
        off = WALL_OFFSET_CS.load(Ordering::Relaxed);
    }
    (off + up_cs) as f64 / 100.0
}

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

    // gfx_pending() -> i32: folds the PS/2 mouse queue and returns the number of
    // queued GUI events. Lets the app skip rendering when nothing changed.
    linker.func_wrap("ruos_gfx", "gfx_pending",
        |_caller: Caller<'_, WtState>| -> i32 {
            crate::gfx::fold_mouse();
            crate::gfx::pending() as i32
        })?;

    // gfx_debug(ptr, len): log a UTF-8 string straight to the kernel serial
    // (bypasses the PTY, which isn't drained while a sync GUI owns the executor).
    linker.func_wrap("ruos_gfx", "gfx_debug",
        |mut caller: Caller<'_, WtState>, ptr: i32, len: i32| {
            if let Some(b) = mem::read(&mut caller, ptr as u32, len as u32) {
                if let Ok(s) = core::str::from_utf8(&b) {
                    crate::kprintln!("[gui] {}", s);
                }
            }
        })?;

    // gfx_wall_secs() -> f64: MONOTONIC seconds (with 10 ms fraction) since local
    // midnight. Feeds egui RawInput.time (must be monotonic) + the desktop clock
    // (Platform::wall_clock_secs). See wall_secs() for why it is latched.
    linker.func_wrap("ruos_gfx", "gfx_wall_secs",
        |_caller: Caller<'_, WtState>| -> f64 { wall_secs() })?;

    Ok(())
}
