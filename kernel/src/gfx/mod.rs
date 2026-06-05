//! Framebuffer GUI service. Lends the Limine framebuffer to a fullscreen GUI
//! app (a `.cwasm` on Wasmtime) while the text console is suspended. Pixels in
//! from the guest are RGBA8888; converted to the panel layout on blit. Input
//! (keyboard scancodes + absolute mouse) is coalesced into a GUI event queue.
//!
//! ABI matches `ruos-desktop/gui-core/src/abi.rs` (the GUI is developed/tested
//! on PC then ported here): GfxInfo = 4×u32 {w,h,stride,format=0(RGBA8888)};
//! events carry raw PS/2 scancodes (extended = 0xE0NN), absolute mouse in f32.

use core::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU32, AtomicU64, Ordering};
use alloc::collections::VecDeque;
use crate::console::fb::{FbInfo, PixelLayout};

pub static GUI_MODE: AtomicBool = AtomicBool::new(false);

// The REAL framebuffer geometry, captured once at device init. Kept independent
// of `console::fb`'s statics because the boot-checks `engine_test` constructs a
// throwaway FramebufferConsole on a synthetic surface that clobbers those.
static GFX_VIRT:  AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
static GFX_PITCH: AtomicU32 = AtomicU32::new(0);
static GFX_BPP:   AtomicU32 = AtomicU32::new(0);
static GFX_W:     AtomicU32 = AtomicU32::new(0);
static GFX_H:     AtomicU32 = AtomicU32::new(0);
static GFX_FMT:   AtomicU32 = AtomicU32::new(0); // 0 = RGB, 1 = BGR

/// Capture the real framebuffer geometry. Called once in the devices boot phase.
pub fn init(info: FbInfo) {
    GFX_VIRT.store(info.addr, Ordering::Release);
    GFX_PITCH.store(info.pitch, Ordering::Release);
    GFX_BPP.store(info.bpp, Ordering::Release);
    GFX_W.store(info.width, Ordering::Release);
    GFX_H.store(info.height, Ordering::Release);
    GFX_FMT.store(match info.pixel { PixelLayout::Rgb => 0, PixelLayout::Bgr => 1 }, Ordering::Release);
    crate::kprintln!(
        "ruos: gfx init {}x{} pitch={} bpp={} fmt={}",
        info.width, info.height, info.pitch, info.bpp,
        match info.pixel { PixelLayout::Rgb => "RGB", PixelLayout::Bgr => "BGR" },
    );
}

/// Canonical app-side pixel format (matches abi::GFX_FORMAT_RGBA8888 = 0).
pub const FORMAT_RGBA8888: u32 = 0;

#[inline]
pub fn gui_mode() -> bool { GUI_MODE.load(Ordering::Acquire) }

/// Enter GUI mode: the console stops painting; the GUI owns the framebuffer.
/// Centre the software mouse cursor so it's visible from the first frame.
pub fn enter() {
    GUI_MODE.store(true, Ordering::Release);
    let sw = GFX_W.load(Ordering::Acquire) as i32;
    let sh = GFX_H.load(Ordering::Acquire) as i32;
    MOUSE_X.store(sw / 2, Ordering::Relaxed);
    MOUSE_Y.store(sh / 2, Ordering::Relaxed);
    CUR_X.store(sw / 2, Ordering::Relaxed);
    CUR_Y.store(sh / 2, Ordering::Relaxed);
    CUR_VALID.store(false, Ordering::Release);
}

/// Leave GUI mode: re-enable the console and clear the screen so the next shell
/// prompt repaints cleanly.
pub fn leave() {
    if !GUI_MODE.swap(false, Ordering::AcqRel) {
        return;
    }
    cursor_erase();
    let mut c = crate::console::CONSOLE.lock();
    if let Some(fb) = &mut c.fb {
        fb.clear();
    }
}

#[derive(Copy, Clone)]
pub struct GfxGeom { pub width: u32, pub height: u32, pub stride: u32, pub format: u32 }

pub fn geom() -> GfxGeom {
    GfxGeom {
        width: GFX_W.load(Ordering::Acquire),
        height: GFX_H.load(Ordering::Acquire),
        stride: GFX_PITCH.load(Ordering::Acquire),
        format: FORMAT_RGBA8888,
    }
}

// Diagnostics so a boot-check can confirm the host fn path ran without racing
// the console redraw on leave().
static BLIT_COUNT: AtomicU64 = AtomicU64::new(0);
static LAST_PIXEL: AtomicU32 = AtomicU32::new(0);

pub fn blit_count() -> u64 { BLIT_COUNT.load(Ordering::Relaxed) }
pub fn last_pixel() -> u32 { LAST_PIXEL.load(Ordering::Relaxed) }

/// Blit a guest RGBA8888 rectangle (`buf`, row-major, `w*h*4` bytes) to the
/// framebuffer at (x,y), converting to the panel layout. Clips to the screen.
pub fn blit(buf: &[u8], x: u32, y: u32, w: u32, h: u32) {
    let base = GFX_VIRT.load(Ordering::Acquire);
    if base.is_null() { return; }
    let pitch = GFX_PITCH.load(Ordering::Acquire) as usize;
    let bpp = (GFX_BPP.load(Ordering::Acquire) as usize) / 8;
    let bgr = GFX_FMT.load(Ordering::Acquire) == 1;
    let sw = GFX_W.load(Ordering::Acquire);
    let sh = GFX_H.load(Ordering::Acquire);
    if w == 0 || h == 0 { return; }

    // Lift the software cursor BEFORE mutating the framebuffer: this restores the
    // true background it was covering, so the post-blit cursor_repaint() saves
    // real pixels, never the sprite. Critical with dirty-rect partial blits that
    // usually do NOT overlap the cursor — otherwise the saved "background" would
    // be the sprite itself and the next cursor move would leave a trail of arrows
    // (denser the slower you move). No-op when no cursor is currently drawn.
    cursor_erase();

    if buf.len() >= 4 {
        LAST_PIXEL.store(
            u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
            Ordering::Relaxed,
        );
    }
    BLIT_COUNT.fetch_add(1, Ordering::Relaxed);

    // Clip the source rect to the screen ONCE (instead of per pixel).
    if x >= sw || y >= sh { cursor_repaint(); return; }
    let vis_w = core::cmp::min(w, sw - x) as usize;
    let vis_h = core::cmp::min(h, sh - y) as usize;
    let src_stride = (w as usize) * 4; // guest is always RGBA8888

    // Fast path: 32-bpp framebuffer. A whole visible row is contiguous on both
    // sides, so RGB blits with a single row memcpy and BGR with a tight per-pixel
    // swap — no per-pixel screen-bounds branch. (egui blits the full screen every
    // frame; the old per-pixel loop with per-pixel clamps was the hot host cost.)
    if bpp == 4 {
        for row in 0..vis_h {
            let src_off = row * src_stride;
            let end = src_off + vis_w * 4;
            if end > buf.len() { break; }
            let dst_off = (y as usize + row) * pitch + (x as usize) * 4;
            let src_row = &buf[src_off..end];
            // SAFETY: dst_off + vis_w*4 ≤ (y+row+1)*pitch ≤ sh*pitch (clipped above);
            // framebuffer is 4 bpp.
            let dst_row = unsafe {
                core::slice::from_raw_parts_mut(base.add(dst_off), vis_w * 4)
            };
            if !bgr {
                // RGBA src → RGBX dst: identical byte order (alpha lands in the
                // ignored X slot). One memcpy per row.
                dst_row.copy_from_slice(src_row);
            } else {
                // BGR fb: swap R/B per pixel; leave the X byte untouched.
                for px in 0..vis_w {
                    let s = px * 4;
                    dst_row[s] = src_row[s + 2];
                    dst_row[s + 1] = src_row[s + 1];
                    dst_row[s + 2] = src_row[s];
                }
            }
        }
        cursor_repaint();
        return;
    }

    // Slow path: non-32-bpp framebuffer (rare) — per-pixel 3-byte writes.
    for row in 0..vis_h {
        let dy = y as usize + row;
        for col in 0..vis_w {
            let dx = x as usize + col;
            let si = (row * w as usize + col) * 4;
            if si + 3 >= buf.len() { cursor_repaint(); return; }
            let (r, g, b) = (buf[si], buf[si + 1], buf[si + 2]);
            let off = dy * pitch + dx * bpp;
            // SAFETY: (dx,dy) is within the framebuffer (clipped above).
            unsafe {
                let p = base.add(off);
                if bgr { *p = b; *p.add(1) = g; *p.add(2) = r; }
                else   { *p = r; *p.add(1) = g; *p.add(2) = b; }
            }
        }
    }
    // The full-frame blit overwrote the cursor — repaint it on top.
    cursor_repaint();
}

// --- Input ---------------------------------------------------------------

/// One GUI event in wire form (16 bytes, 4×u32 LE): [kind, p0, p1, p2].
/// kind 0 key {p0=scancode, p1=pressed}, 1 mousemove {p0=x f32, p1=y f32},
/// 2 mousebtn {p0=button 0=L/1=R/2=M, p1=pressed}, 3 resize {p0=w,p1=h}, 4 quit.
#[derive(Copy, Clone)]
pub struct GfxEvt { pub kind: u32, pub p0: u32, pub p1: u32, pub p2: u32 }

const QUEUE_CAP: usize = 512;
static EVENTS: crate::sync::IrqMutex<VecDeque<GfxEvt>> =
    crate::sync::IrqMutex::new(VecDeque::new());

fn push(ev: GfxEvt) {
    let mut q = EVENTS.lock();
    if q.len() >= QUEUE_CAP { q.pop_front(); }
    q.push_back(ev);
}

/// Push a keyboard event (called from the keyboard ISR in GUI mode). `scancode`
/// is the raw PS/2 set-1 code; extended keys are pre-encoded as 0xE0NN.
pub fn push_key(scancode: u32, pressed: bool) {
    push(GfxEvt { kind: 0, p0: scancode, p1: pressed as u32, p2: 0 });
}

/// Drain one GUI event.
pub fn pop() -> Option<GfxEvt> { EVENTS.lock().pop_front() }

/// Number of queued GUI events (peek, for on-demand repaint).
pub fn pending() -> usize { EVENTS.lock().len() }

// Absolute cursor position + previous button state (for edge detection).
static MOUSE_X: AtomicI32 = AtomicI32::new(0);
static MOUSE_Y: AtomicI32 = AtomicI32::new(0);
static BTN_L: AtomicBool = AtomicBool::new(false);
static BTN_R: AtomicBool = AtomicBool::new(false);
static BTN_M: AtomicBool = AtomicBool::new(false);

/// Drain the raw mouse queue, fold deltas into an absolute position (clamped to
/// the screen), and emit MouseMove/MouseButton GUI events.
///
/// Called by the GUI every frame. A sync GUI owns the executor and never yields,
/// so the async `usb_poll_task` is starved while it runs — USB is polled, not
/// interrupt-driven, so without this the USB keyboard/mouse go dead in the GUI
/// (PS/2 keeps working because it is IRQ-driven). Pump the USB controller here so
/// USB HID reports are serviced and injected before we fold them.
pub fn fold_mouse() {
    crate::usb::poll();
    let sw = GFX_W.load(Ordering::Acquire) as i32;
    let sh = GFX_H.load(Ordering::Acquire) as i32;
    let mut moved = false;
    while let Some(ev) = crate::mouse::pop_event() {
        let ox = MOUSE_X.load(Ordering::Relaxed);
        let oy = MOUSE_Y.load(Ordering::Relaxed);
        let nx = (ox + ev.dx as i32).clamp(0, (sw - 1).max(0));
        let ny = (oy + ev.dy as i32).clamp(0, (sh - 1).max(0));
        if nx != ox || ny != oy {
            MOUSE_X.store(nx, Ordering::Relaxed);
            MOUSE_Y.store(ny, Ordering::Relaxed);
            moved = true;
            push(GfxEvt {
                kind: 1,
                p0: (nx as f32).to_bits(),
                p1: (ny as f32).to_bits(),
                p2: 0,
            });
        }
        for (i, (cur, prev)) in [
            (ev.left, &BTN_L), (ev.right, &BTN_R), (ev.middle, &BTN_M),
        ].iter().enumerate() {
            if *cur != prev.load(Ordering::Relaxed) {
                prev.store(*cur, Ordering::Relaxed);
                push(GfxEvt { kind: 2, p0: i as u32, p1: *cur as u32, p2: 0 });
            }
        }
    }
    // Responsive software cursor: redraw at the new position without forcing a
    // (slow) full-frame egui re-render.
    if moved {
        cursor_move(MOUSE_X.load(Ordering::Relaxed), MOUSE_Y.load(Ordering::Relaxed));
    }
}

// --- Software mouse cursor -----------------------------------------------
// A small arrow drawn directly on the framebuffer in GUI mode. Responsive
// (redrawn on each mouse move, no egui re-render) and composited over the
// full-frame blit. Save/restore the background under the sprite so moving it
// leaves no trail. Assumes 32-bpp framebuffer (GFX_BPP=32); black/white sprite
// pixels are RGB/BGR-agnostic.

const CUR_W: usize = 12;
const CUR_H: usize = 19;
// ' ' = transparent, 'X' = black outline, '.' = white fill.
const CUR: [&[u8; CUR_W]; CUR_H] = [
    b"X           ",
    b"XX          ",
    b"X.X         ",
    b"X..X        ",
    b"X...X       ",
    b"X....X      ",
    b"X.....X     ",
    b"X......X    ",
    b"X.......X   ",
    b"X........X  ",
    b"X.........X ",
    b"X......XXXXX",
    b"X...X..X    ",
    b"X..XX..X    ",
    b"X.X  X..X   ",
    b"XX   X..X   ",
    b"      X..X  ",
    b"      X..X  ",
    b"      XXXX  ",
];

static CUR_X: AtomicI32 = AtomicI32::new(0);
static CUR_Y: AtomicI32 = AtomicI32::new(0);
static CUR_VALID: AtomicBool = AtomicBool::new(false);
static CUR_SAVE: crate::sync::IrqMutex<[u32; CUR_W * CUR_H]> =
    crate::sync::IrqMutex::new([0; CUR_W * CUR_H]);

/// Restore the framebuffer pixels saved under the cursor (erase it).
fn cursor_erase() {
    if !CUR_VALID.swap(false, Ordering::AcqRel) {
        return;
    }
    let base = GFX_VIRT.load(Ordering::Acquire);
    if base.is_null() {
        return;
    }
    let pitch = GFX_PITCH.load(Ordering::Acquire) as usize;
    let sw = GFX_W.load(Ordering::Acquire) as i32;
    let sh = GFX_H.load(Ordering::Acquire) as i32;
    let cx = CUR_X.load(Ordering::Relaxed);
    let cy = CUR_Y.load(Ordering::Relaxed);
    let save = CUR_SAVE.lock();
    for row in 0..CUR_H as i32 {
        let py = cy + row;
        if py < 0 || py >= sh { continue; }
        for col in 0..CUR_W as i32 {
            let px = cx + col;
            if px < 0 || px >= sw { continue; }
            let off = (py as usize) * pitch + (px as usize) * 4;
            // SAFETY: (px,py) is within the framebuffer.
            unsafe { *(base.add(off) as *mut u32) = save[(row as usize) * CUR_W + col as usize]; }
        }
    }
}

/// Save the background under (x,y) and draw the cursor sprite. Assumes the
/// previous cursor has already been erased (or its save buffer invalidated).
fn cursor_paint(x: i32, y: i32) {
    let base = GFX_VIRT.load(Ordering::Acquire);
    if base.is_null() {
        return;
    }
    let pitch = GFX_PITCH.load(Ordering::Acquire) as usize;
    let sw = GFX_W.load(Ordering::Acquire) as i32;
    let sh = GFX_H.load(Ordering::Acquire) as i32;
    let mut save = CUR_SAVE.lock();
    for row in 0..CUR_H as i32 {
        let py = y + row;
        for col in 0..CUR_W as i32 {
            let px = x + col;
            let idx = (row as usize) * CUR_W + col as usize;
            if px < 0 || px >= sw || py < 0 || py >= sh {
                save[idx] = 0;
                continue;
            }
            let off = (py as usize) * pitch + (px as usize) * 4;
            // SAFETY: (px,py) is within the framebuffer.
            let p = unsafe { base.add(off) as *mut u32 };
            save[idx] = unsafe { *p };
            match CUR[row as usize][col as usize] {
                b'X' => unsafe { *p = 0x0000_0000; }, // black outline
                b'.' => unsafe { *p = 0x00FF_FFFF; }, // white fill
                _ => {}                                // transparent
            }
        }
    }
    CUR_X.store(x, Ordering::Relaxed);
    CUR_Y.store(y, Ordering::Relaxed);
    CUR_VALID.store(true, Ordering::Release);
}

/// Move the cursor to (x,y): erase the old, paint the new. GUI-mode only.
pub fn cursor_move(x: i32, y: i32) {
    if !gui_mode() { return; }
    cursor_erase();
    cursor_paint(x, y);
}

/// Repaint the cursor at its current position after a blit. `blit()` calls
/// `cursor_erase()` BEFORE it writes, so the framebuffer under the cursor is the
/// true background (sprite already lifted) when `cursor_paint` saves it here —
/// never the sprite. Without that pre-erase, a partial (dirty-rect) blit that
/// does not cover the cursor would save the sprite as background, and the next
/// cursor move would restore it → a trail of arrows (worse the slower you move).
fn cursor_repaint() {
    if !gui_mode() { return; }
    cursor_paint(CUR_X.load(Ordering::Relaxed), CUR_Y.load(Ordering::Relaxed));
}

/// Boot-check: blit a 2×2 red square at (0,0) and read pixel (0,0) back.
#[cfg(feature = "boot-checks")]
pub fn self_test() -> bool {
    // 2×2 red RGBA8888.
    let red = [0xFFu8, 0x00, 0x00, 0xFF];
    let mut buf = [0u8; 16];
    for px in buf.chunks_mut(4) { px.copy_from_slice(&red); }
    blit(&buf, 0, 0, 2, 2);
    let base = GFX_VIRT.load(Ordering::Acquire);
    if base.is_null() { return false; }
    let _bpp = (GFX_BPP.load(Ordering::Acquire) as usize) / 8;
    let bgr = GFX_FMT.load(Ordering::Acquire) == 1;
    // SAFETY: pixel (0,0) is within the framebuffer.
    let (b0, b1, b2) = unsafe { (*base, *base.add(1), *base.add(2)) };
    if bgr { b0 == 0x00 && b1 == 0x00 && b2 == 0xFF } else { b0 == 0xFF && b1 == 0x00 && b2 == 0x00 }
}
