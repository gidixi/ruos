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
}

/// Canonical app-side pixel format (matches abi::GFX_FORMAT_RGBA8888 = 0).
pub const FORMAT_RGBA8888: u32 = 0;

#[inline]
pub fn gui_mode() -> bool { GUI_MODE.load(Ordering::Acquire) }

/// Enter GUI mode: the console stops painting; the GUI owns the framebuffer.
pub fn enter() { GUI_MODE.store(true, Ordering::Release); }

/// Leave GUI mode: re-enable the console and clear the screen so the next shell
/// prompt repaints cleanly.
pub fn leave() {
    if !GUI_MODE.swap(false, Ordering::AcqRel) {
        return;
    }
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
    if w > 0 && h > 0 {
        let i0 = 0usize;
        if i0 + 3 < buf.len() {
            LAST_PIXEL.store(
                u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
                Ordering::Relaxed,
            );
        }
        BLIT_COUNT.fetch_add(1, Ordering::Relaxed);
    }
    for row in 0..h {
        let dy = y + row;
        if dy >= sh { break; }
        for col in 0..w {
            let dx = x + col;
            if dx >= sw { continue; }
            let si = ((row * w + col) * 4) as usize;
            if si + 3 >= buf.len() { return; }
            let (r, g, b) = (buf[si], buf[si + 1], buf[si + 2]);
            let off = (dy as usize) * pitch + (dx as usize) * bpp;
            // SAFETY: off is within the framebuffer (bounds-checked above).
            unsafe {
                let p = base.add(off);
                if bgr {
                    *p = b; *p.add(1) = g; *p.add(2) = r;
                } else {
                    *p = r; *p.add(1) = g; *p.add(2) = b;
                }
            }
        }
    }
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

// Absolute cursor position + previous button state (for edge detection).
static MOUSE_X: AtomicI32 = AtomicI32::new(0);
static MOUSE_Y: AtomicI32 = AtomicI32::new(0);
static BTN_L: AtomicBool = AtomicBool::new(false);
static BTN_R: AtomicBool = AtomicBool::new(false);
static BTN_M: AtomicBool = AtomicBool::new(false);

/// Drain the raw PS/2 mouse queue, fold deltas into an absolute position
/// (clamped to the screen), and emit MouseMove/MouseButton GUI events.
pub fn fold_mouse() {
    let sw = GFX_W.load(Ordering::Acquire) as i32;
    let sh = GFX_H.load(Ordering::Acquire) as i32;
    while let Some(ev) = crate::mouse::pop_event() {
        let ox = MOUSE_X.load(Ordering::Relaxed);
        let oy = MOUSE_Y.load(Ordering::Relaxed);
        let nx = (ox + ev.dx as i32).clamp(0, (sw - 1).max(0));
        let ny = (oy + ev.dy as i32).clamp(0, (sh - 1).max(0));
        if nx != ox || ny != oy {
            MOUSE_X.store(nx, Ordering::Relaxed);
            MOUSE_Y.store(ny, Ordering::Relaxed);
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
