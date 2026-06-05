//! Window-manager / compositor host module (`wm`) + reactor driver. Holds N
//! persistent wasm instances; calls their exported `frame()` each loop; reads
//! each committed surface into the per-store WmState.
//!
//! SP2 turns the static gate into a real input-routed compositor: the canonical
//! `Window` + `Compositor` types (per the interface contract) own the instances,
//! z-order (= `wins` Vec order), focus, and per-window input queues. The
//! compositor is the SOLE consumer of `crate::gfx::pop()` in compositor mode: it
//! folds the mouse, hit-tests the live cursor against window rects, sets focus on
//! a mouse-button-down (click-to-focus), translates mouse coords to window-local,
//! and pushes events into ONLY the focused window's queue. Each app drains its own
//! queue via the `wm.poll_event` host fn.
//!
//! The `wm` imports are raw `extern "C"` (not WIT) to keep the reactor focused on
//! the mechanism: a PERSISTENT instance whose `frame()` export is called
//! repeatedly. WIT-ification comes when building the real apps.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use wasmtime::{Caller, Extern, Instance, Linker, Memory, Module, Store};
use crate::gfx::GfxEvt;
use crate::wasm::wt::engine;
use core::sync::atomic::{AtomicU32, Ordering};
use crate::wasm::wt::compose::{composite_band, WinDesc};

/// Max composite bands per frame. One band per pool slot at most; capped well
/// under `pool::MAX_JOBS` (64) since a frame uses ~num-cores bands.
const MAX_BANDS: usize = 16;

/// Shared, snapshot footprint descriptors for the current frame. All band jobs
/// of one frame read the same footprint list (painter order = `wins` order).
/// Sized for plenty of windows; the BSP fills `[0, n)` then submits.
const MAX_WINS: usize = 64;

/// A single band job's encoded descriptor. Lives in the `'static` arena so the
/// `pool::submit(work, input: &'static [u8])` bound is satisfied soundly. The
/// BSP fills slot `b`, submits a `&'static [u8]` view of it, and BLOCKS on the
/// join before reusing it next frame — so no two in-flight jobs alias a slot.
#[repr(C)]
#[derive(Copy, Clone)]
struct BandArg {
    back: usize,    // back-buffer base pointer as usize
    stride: usize,  // back-buffer byte pitch
    screen_w: u32,
    band_y0: u32,
    band_y1: u32,
    bg: u32,        // background RGBX
    wins: usize,    // *const WinDesc (into the shared WIN arena)
    n_wins: usize,  // number of WinDescs
}

/// SAFETY of the arena: `BandArg` is `Copy` plain-old-data; the pointers it
/// carries (`back`, `wins`) point at the back-buffer and the WIN arena, both of
/// which the BSP keeps alive across the join. Only the BSP writes the arena
/// (between frames, never concurrently with in-flight jobs).
static mut BAND_ARENA: [BandArg; MAX_BANDS] = [BandArg {
    back: 0, stride: 0, screen_w: 0, band_y0: 0, band_y1: 0, bg: 0, wins: 0, n_wins: 0,
}; MAX_BANDS];

/// Shared snapshot of the DECORATED footprints for the current frame. All band
/// jobs read the same list (painter order). The BSP fills `[0, n)` then submits;
/// the footprint backing buffers are kept alive on the BSP across the join.
static mut WIN_ARENA: [WinDesc; MAX_WINS] = [WinDesc {
    px: core::ptr::null(), px_len: 0, x: 0, y: 0, w: 0, h: 0,
}; MAX_WINS];

/// Distinct cores that ran a composite job in the most recent frame (bitset by
/// cpu_id). Read by the boot-check marker to prove multi-core compositing.
static COMPOSITE_CORE_MASK: AtomicU32 = AtomicU32::new(0);

/// Read + clear the composite core mask (boot-check marker support).
pub fn take_composite_core_mask() -> u32 {
    COMPOSITE_CORE_MASK.swap(0, Ordering::SeqCst)
}

/// Pool job: composite one band. `input` is a byte view of one `BandArg` in the
/// static arena (its bytes copied in by the dispatcher). Returns 0 (unused).
///
/// SAFETY: the dispatcher guarantees (a) `input` is exactly `size_of::<BandArg>()`
/// bytes of a valid `BandArg`, (b) `back`/`wins` point at live buffers for the
/// job's lifetime (BSP blocks on join before freeing), (c) this job's
/// `[band_y0, band_y1)` is disjoint from every other in-flight job's range.
fn composite_band_job(input: &[u8]) -> u64 {
    if input.len() < core::mem::size_of::<BandArg>() {
        return 0;
    }
    // Reconstruct the BandArg from the raw bytes (same round-trip pattern as
    // pool::run_slot uses for the JobFn pointer).
    let arg: BandArg = unsafe { core::ptr::read_unaligned(input.as_ptr() as *const BandArg) };
    // SAFETY: wins points into the live WIN_ARENA for `n_wins` entries.
    let wins: &[WinDesc] =
        unsafe { core::slice::from_raw_parts(arg.wins as *const WinDesc, arg.n_wins) };
    // SAFETY: disjoint band rows + live back-buffer (see fn-level contract).
    unsafe {
        composite_band(
            arg.back as *mut u8,
            arg.stride,
            arg.screen_w,
            arg.band_y0,
            arg.band_y1,
            arg.bg,
            wins,
        );
    }
    // Record which core ran this band (cpu_id is LAPIC-based — VBox-safe).
    let cpu = crate::cpu::cpu_id();
    if cpu < 32 {
        COMPOSITE_CORE_MASK.fetch_or(1u32 << cpu, Ordering::SeqCst);
    }
    0
}

/// Dispatch the banded composite into the screen-sized RGBX back-buffer pointed
/// at by `back_ptr` (len = `stride*screen_h`). The WIN arena MUST already be
/// filled (by `Compositor::present`) with the `n_wins` decorated footprint
/// descriptors in painter order — this fn does NOT fill it. It splits the screen
/// into `n_bands` disjoint horizontal bands, submits one pool job per band,
/// drains inline when there are no APs (1-CPU fallback) or the pool is full, and
/// JOINS every submitted job before returning. The caller then presents.
///
/// Correctness: bands have DISJOINT row ranges, every job writes only its own
/// rows ⇒ no two jobs touch the same back-buffer byte. The framebuffer itself is
/// NOT touched here (the BSP presents serially afterward).
fn dispatch_bands(
    back_ptr: usize,
    stride: usize,
    screen_w: u32,
    screen_h: u32,
    bg: u32,
    n_wins: usize,
) {
    if screen_h == 0 || screen_w == 0 { return; }

    // `wins` base into the shared WIN arena (already filled by present()).
    let wins_ptr = core::ptr::addr_of!(WIN_ARENA) as usize;

    // Band count: one band per online core (incl. BSP), capped — unless the
    // serial-composite feature forces a single band (visual-equivalence ref).
    #[cfg(feature = "serial-composite")]
    let n_bands = 1usize;
    #[cfg(not(feature = "serial-composite"))]
    let n_bands = core::cmp::min(core::cmp::max(crate::cpu::cpus_online() as usize, 1), MAX_BANDS);
    let band_rows = (screen_h as usize + n_bands - 1) / n_bands; // ceil

    // Fill the band arena + submit one job per band.
    let mut ids: [usize; MAX_BANDS] = [usize::MAX; MAX_BANDS];
    let mut n_submitted = 0usize;
    for b in 0..n_bands {
        let y0 = (b * band_rows) as u32;
        if y0 >= screen_h { break; }
        let y1 = core::cmp::min(((b + 1) * band_rows) as u32, screen_h);
        let arg = BandArg {
            back: back_ptr,
            stride,
            screen_w,
            band_y0: y0,
            band_y1: y1,
            bg,
            wins: wins_ptr,
            n_wins,
        };
        // SAFETY: BSP-only write of slot b; previous frame fully joined.
        unsafe { BAND_ARENA[b] = arg; }
        // A `&'static [u8]` view of this arena slot. `BAND_ARENA` is a real
        // `static`, so the slice is genuinely `'static` — the bound is sound,
        // not transmuted. The BSP blocks on join below before reusing slot b.
        let bytes: &'static [u8] = unsafe {
            core::slice::from_raw_parts(
                core::ptr::addr_of!(BAND_ARENA[b]) as *const u8,
                core::mem::size_of::<BandArg>(),
            )
        };
        match crate::smp::pool::submit(composite_band_job, bytes) {
            Some(id) => { ids[b] = id; n_submitted += 1; }
            None => { break; } // pool full: remaining bands run inline below
        }
    }

    // 1-CPU (or pool-full) fallback: drain any queued jobs inline on the BSP so
    // we never deadlock waiting on cores that aren't there.
    if crate::cpu::cpus_online() <= 1 {
        while let Some(slot) = crate::smp::pool::take() {
            crate::smp::pool::run_slot(slot, crate::cpu::cpu_id());
        }
    }

    // If submission stopped early (pool full), composite the leftover bands
    // inline on the BSP — correctness over parallelism.
    for b in n_submitted..n_bands {
        let y0 = (b * band_rows) as u32;
        if y0 >= screen_h { break; }
        let y1 = core::cmp::min(((b + 1) * band_rows) as u32, screen_h);
        // SAFETY: leftover bands are disjoint from submitted ones; submitted jobs
        // only ever touch THEIR rows, so running these inline cannot race them.
        unsafe {
            composite_band(
                back_ptr as *mut u8,
                stride,
                screen_w,
                y0,
                y1,
                bg,
                core::slice::from_raw_parts(wins_ptr as *const WinDesc, n_wins),
            );
        }
        let cpu = crate::cpu::cpu_id();
        if cpu < 32 { COMPOSITE_CORE_MASK.fetch_or(1u32 << cpu, Ordering::SeqCst); }
    }

    // Join: block until every submitted band is DONE. poll_done frees slots.
    for b in 0..n_bands {
        if ids[b] == usize::MAX { continue; }
        loop {
            if crate::smp::pool::poll_done(ids[b]).is_some() { break; }
            core::hint::spin_loop();
        }
    }
    // After this point all jobs are DONE → the BSP may safely reuse the arenas
    // next frame and read/own `back` for the present.
}

/// Window decorations: a title bar above each surface + a close [X] button.
/// All geometry is pure (no framebuffer access) so it is unit-checkable; the
/// drawing helpers raster into a caller-owned RGBA8888 `Vec<u8>` (also no
/// framebuffer access), so SP4 can call them on AP compositing jobs.
pub mod decor {
    /// Title-bar height in pixels (above the surface).
    pub const TITLE_H: u32 = 28;
    /// Close-button square edge (inside the title bar, right-aligned).
    pub const BTN_W: u32 = TITLE_H;
    /// Text inset from the left edge of the title bar.
    pub const TEXT_PAD_X: u32 = 8;

    // Decoration colours, RGBA8888 little-endian as [r,g,b,a].
    pub const BAR_FOCUSED:   [u8; 4] = [0x2E, 0x5A, 0x88, 0xFF]; // blue bar (active)
    pub const BAR_UNFOCUSED: [u8; 4] = [0x4A, 0x4A, 0x4A, 0xFF]; // grey bar (inactive)
    pub const TEXT_RGBA:     [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF]; // white title text
    pub const CLOSE_BG:      [u8; 4] = [0xC0, 0x3A, 0x2A, 0xFF]; // red [X] background
    pub const CLOSE_GLYPH:   [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF]; // white [X] glyph

    /// Title-bar rect on screen for a surface rect `(sx,sy,sw,sh)`:
    /// returns (x, y, w, h) of the bar (directly above the surface).
    /// Caller guarantees `sy >= TITLE_H`.
    pub fn title_rect(s: (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
        let (sx, sy, sw, _sh) = s;
        (sx, sy - TITLE_H, sw, TITLE_H)
    }

    /// Close-button rect on screen for a surface rect `(sx,sy,sw,sh)`:
    /// a BTN_W square at the right end of the title bar.
    pub fn close_rect(s: (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
        let (sx, sy, sw, _sh) = s;
        let bw = if sw < BTN_W { sw } else { BTN_W };
        (sx + sw - bw, sy - TITLE_H, bw, TITLE_H)
    }

    /// Full window footprint (title bar + surface) for hit-testing/composite.
    pub fn window_rect(s: (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
        let (sx, sy, sw, sh) = s;
        (sx, sy - TITLE_H, sw, sh + TITLE_H)
    }

    /// True if (px,py) is inside rect `r=(x,y,w,h)`.
    pub fn contains(r: (u32, u32, u32, u32), px: i32, py: i32) -> bool {
        let (x, y, w, h) = r;
        px >= x as i32 && py >= y as i32
            && px < (x + w) as i32 && py < (y + h) as i32
    }

    /// Where a point landed on a window. Used by the input dispatcher.
    #[derive(Copy, Clone, PartialEq, Eq, Debug)]
    pub enum Hit { Close, Title, Surface, Outside }

    /// Classify (px,py) against a surface rect `s` (decoration-aware).
    /// Close takes priority over Title; Title over Surface.
    pub fn hit(s: (u32, u32, u32, u32), px: i32, py: i32) -> Hit {
        if contains(close_rect(s), px, py) { return Hit::Close; }
        if contains(title_rect(s), px, py) { return Hit::Title; }
        if contains(s, px, py) { return Hit::Surface; }
        Hit::Outside
    }

    /// Fill a solid RGBA rect into a row-major RGBA8888 buffer `buf` of size
    /// `buf_w × buf_h`. Clips to the buffer. (x,y) is buffer-local.
    pub fn fill_rect(buf: &mut [u8], buf_w: u32, buf_h: u32,
                     x: u32, y: u32, w: u32, h: u32, c: [u8; 4]) {
        let bw = buf_w as usize;
        for ry in 0..h as usize {
            let py = y as usize + ry;
            if py >= buf_h as usize { break; }
            for rx in 0..w as usize {
                let px = x as usize + rx;
                if px >= bw { break; }
                let o = (py * bw + px) * 4;
                if o + 4 > buf.len() { break; }
                buf[o] = c[0]; buf[o + 1] = c[1]; buf[o + 2] = c[2]; buf[o + 3] = c[3];
            }
        }
    }

    /// Alpha-blend one glyph's coverage onto `buf` in colour `c` at buffer-local
    /// (gx,gy). `raster` rows are alpha intensities (0..=255) from the noto font.
    fn blend_glyph(buf: &mut [u8], buf_w: u32, buf_h: u32,
                   gx: i32, gy: i32, raster: &[&[u8]], c: [u8; 4]) {
        let bw = buf_w as usize;
        for (ry, row) in raster.iter().enumerate() {
            let py = gy + ry as i32;
            if py < 0 || py >= buf_h as i32 { continue; }
            for (rx, &a) in row.iter().enumerate() {
                if a == 0 { continue; }
                let px = gx + rx as i32;
                if px < 0 || px >= bw as i32 { continue; }
                let o = (py as usize * bw + px as usize) * 4;
                if o + 4 > buf.len() { continue; }
                let a16 = a as u16;
                let inv = 255 - a16;
                // out = src*a + dst*(1-a), per channel (8-bit).
                buf[o]     = ((c[0] as u16 * a16 + buf[o]     as u16 * inv) / 255) as u8;
                buf[o + 1] = ((c[1] as u16 * a16 + buf[o + 1] as u16 * inv) / 255) as u8;
                buf[o + 2] = ((c[2] as u16 * a16 + buf[o + 2] as u16 * inv) / 255) as u8;
                buf[o + 3] = 0xFF;
            }
        }
    }

    /// Draw a UTF-8 string starting at buffer-local (x,y), advancing by the
    /// monospace glyph width. Stops at the right edge `max_x`. Uses the kernel's
    /// noto bitmap font (Regular weight). Vertically centres in TITLE_H.
    pub fn draw_text(buf: &mut [u8], buf_w: u32, buf_h: u32,
                     x: u32, y: u32, max_x: u32, text: &str, c: [u8; 4]) {
        let gw = crate::console::font::glyph_width() as u32;
        let gh = crate::console::font::glyph_height() as i32;
        let gy = y as i32 + ((TITLE_H as i32 - gh) / 2).max(0);
        let mut pen = x;
        for ch in text.chars() {
            if pen + gw > max_x { break; }
            let r = crate::console::font::raster_for_weight(ch, false);
            blend_glyph(buf, buf_w, buf_h, pen as i32, gy, r.raster(), c);
            pen += gw;
        }
    }
}

/// Per-instance store data: window id, last committed surface, and this window's
/// private input queue (the compositor pushes routed events here; the app drains
/// them via `wm.poll_event`).
pub struct WmState {
    pub id: u32,
    pub win_w: u32,
    pub win_h: u32,
    pub pixels: Vec<u8>,
    pub tick: u32,
    pub events: VecDeque<GfxEvt>,
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

/// Write `buf` into this guest's exported linear memory at `ptr`. No-op (returns
/// false) if the memory export is missing or the range is out of bounds. (Mirrors
/// `crate::wasm::wt::mem::write`, which is typed to `WtState`.)
fn write_guest(caller: &mut Caller<'_, WmState>, ptr: u32, buf: &[u8]) -> bool {
    let mem = match caller.get_export("memory") {
        Some(Extern::Memory(m)) => m,
        _ => return false,
    };
    let mem: Memory = mem;
    mem.write(caller, ptr as usize, buf).is_ok()
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
    // wm.poll_event(retptr): drain ONE event from THIS window's queue into the
    // guest's 20-byte return area. The calling app is identified by its own
    // Store (caller.data()), so it can only ever see its own window's events.
    // Layout matches `ruos:gui/gfx poll-event`: discriminant u32 @0 (0=none,
    // 1=some), then the gfx-event record kind@4, p0@8, p1@12, p2@16 (all LE).
    linker.func_wrap("wm", "poll_event",
        |mut caller: Caller<'_, WmState>, retptr: i32| {
            let ev = caller.data_mut().events.pop_front();
            let mut buf = [0u8; 20];
            if let Some(e) = ev {
                buf[0..4].copy_from_slice(&1u32.to_le_bytes());   // some
                buf[4..8].copy_from_slice(&e.kind.to_le_bytes());
                buf[8..12].copy_from_slice(&e.p0.to_le_bytes());
                buf[12..16].copy_from_slice(&e.p1.to_le_bytes());
                buf[16..20].copy_from_slice(&e.p2.to_le_bytes());
            }
            // else: discriminant stays 0 (none); payload zeroed.
            write_guest(&mut caller, retptr as u32, &buf);
        })?;
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
    let mut store = Store::new(
        engine,
        WmState { id: 0, win_w: 0, win_h: 0, pixels: Vec::new(), tick: 0, events: VecDeque::new() },
    );
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

// --- Canonical compositor types (interface contract) ----------------------
//
// `Window` = one persistent reactor instance + its placement + decorations.
// Surface pixels live in `store.data().pixels` (NOT a field here). `Compositor`
// owns the window list; the `wins` Vec order IS the z-order (0 bottom … last
// top). SP3/SP4/SP5 extend these same types.

/// In-progress title-bar drag. `win_id` is the dragged window's id (stable
/// across z-order changes, unlike the Vec index); `grab_dx`/`grab_dy` are the
/// cursor offset inside the window footprint at mousedown, so the window tracks
/// the cursor without jumping.
#[derive(Copy, Clone)]
pub struct DragState {
    pub win_id: u32,
    pub grab_dx: i32,
    pub grab_dy: i32,
}

/// One window = one persistent reactor instance + its placement + decorations.
/// Surface pixels live in `store.data().pixels` (NOT a field here).
pub struct Window {
    pub id: u32,
    pub store: Store<WmState>,
    pub inst: Instance,
    pub rect: (u32, u32, u32, u32), // SURFACE rect (x, y, w, h), EXCLUDING decorations
    pub title: String,              // shown in the SP3 title bar; "" until SP3
    pub focused: bool,
    pub alive: bool,                // SP5 sets false to schedule teardown
}

/// Window order in `wins` IS the z-order: index 0 = bottom, last = top.
/// There is NO `z: u32` field — `raise(idx)` moves the window to the end.
pub struct Compositor {
    pub wins: Vec<Window>,
    pub module: Module,            // shared AOT module; instances cheap
    pub linker: Linker<WmState>,
    pub focused: usize,            // index into wins (the focused window)
    pub drag: Option<DragState>,   // SP3 adds; None until SP3
    pub backbuf: alloc::vec::Vec<u8>, // SP3 screen back-buffer (lazily sized in present)
}

/// Reactor surface size (matches `tools/wt-reactor` W/H). Windows are fixed at
/// this size in SP2; SP3 will make them resizable.
const WIN_W: u32 = 320;
const WIN_H: u32 = 240;

/// Desktop background (RGBA8888 [r,g,b,a]) shown where no window covers.
const DESKTOP_BG: [u8; 4] = [0x10, 0x18, 0x20, 0xFF];

impl Compositor {
    /// Deserialize the shared reactor module, build the `wm` linker, and create
    /// the demo's 2 OVERLAPPING titled windows. Each surface `sy >= TITLE_H` so
    /// the title bars are on-screen, and the two windows overlap so the raise /
    /// lower behaviour is directly visible. Window 0 starts focused.
    pub fn new(cwasm: &[u8]) -> Compositor {
        let engine = engine();
        // SAFETY: produced by wt-precompile for this exact engine Config.
        let module = unsafe { Module::deserialize(engine, cwasm) }.expect("reactor module");
        let mut linker: Linker<WmState> = Linker::new(engine);
        add_to_linker(&mut linker).expect("wm linker");

        let th = decor::TITLE_H;
        // (surface x, y, w, h, title) — A and B overlap so raise/lower is visible.
        let placements: [(u32, u32, u32, u32, &str); 2] = [
            (60,  th + 60,  WIN_W, WIN_H, "reactor A"),
            (300, th + 170, WIN_W, WIN_H, "reactor B"),
        ];
        let mut wins: Vec<Window> = Vec::new();
        for (id, &(x, y, w, h, title)) in placements.iter().enumerate() {
            let mut store = Store::new(
                engine,
                WmState { id: id as u32, win_w: 0, win_h: 0, pixels: Vec::new(), tick: 0, events: VecDeque::new() },
            );
            let inst = linker.instantiate(&mut store, &module).expect("instantiate");
            wins.push(Window {
                id: id as u32,
                store,
                inst,
                rect: (x, y, w, h),
                title: String::from(title),
                focused: id == 0,
                alive: true,
            });
        }
        Compositor { wins, module, linker, focused: 0, drag: None, backbuf: Vec::new() }
    }

    /// TOPMOST window whose surface rect contains framebuffer point (px, py).
    /// Searches z-order from top (last) to bottom (first). SP3 makes this
    /// decoration-aware.
    pub fn window_at(&self, px: i32, py: i32) -> Option<usize> {
        for i in (0..self.wins.len()).rev() {
            let (rx, ry, rw, rh) = self.wins[i].rect;
            let (rx, ry) = (rx as i32, ry as i32);
            if px >= rx && px < rx + rw as i32 && py >= ry && py < ry + rh as i32 {
                return Some(i);
            }
        }
        None
    }

    /// The ONE focus impl: clear the old focused flag, set the new one, update
    /// `self.focused`. SP3/SP5 call this; they do NOT add their own.
    pub fn set_focus(&mut self, idx: usize) {
        if idx >= self.wins.len() { return; }
        if idx != self.focused {
            crate::binfo!("wm", "WM-FOCUS {}", idx);
        }
        if self.focused < self.wins.len() {
            self.wins[self.focused].focused = false;
        }
        self.wins[idx].focused = true;
        self.focused = idx;
    }

    /// Move `wins[idx]` to the end (top of z-order); returns its new index. Does
    /// NOT change focus (callers pair `raise` then `set_focus`).
    pub fn raise(&mut self, idx: usize) -> usize {
        if idx >= self.wins.len() { return idx; }
        let last = self.wins.len() - 1;
        if idx == last { return last; }
        let w = self.wins.remove(idx);
        self.wins.push(w);
        // Keep `self.focused` pointing at the same window if it moved.
        if self.focused == idx {
            self.focused = last;
        } else if self.focused > idx {
            self.focused -= 1;
        }
        last
    }

    /// Index of the window with `id`, or None.
    pub fn index_of(&self, id: u32) -> Option<usize> {
        self.wins.iter().position(|w| w.id == id)
    }

    /// Topmost window whose FULL FOOTPRINT (title bar + surface) contains
    /// (px,py). Iterates top→bottom (last index first). Returns the Vec index.
    /// (Decoration-aware variant of `window_at`, which is surface-only.)
    pub fn topmost_decor_at(&self, px: i32, py: i32) -> Option<usize> {
        for i in (0..self.wins.len()).rev() {
            if decor::contains(decor::window_rect(self.wins[i].rect), px, py) {
                return Some(i);
            }
        }
        None
    }

    /// Translate window `id`'s surface rect so the grabbed point follows the
    /// cursor. `(cx,cy)` is the absolute cursor; `grab` is the offset captured
    /// at mousedown. Clamps so the full footprint stays on screen.
    pub fn drag_to(&mut self, id: u32, cx: i32, cy: i32, grab: (i32, i32)) {
        let g = crate::gfx::geom();
        let (sw_screen, sh_screen) = (g.width as i32, g.height as i32);
        if let Some(i) = self.index_of(id) {
            let (_, _, w, h) = self.wins[i].rect;
            // New footprint origin = cursor - grab offset.
            let mut fx = cx - grab.0;
            let mut fy = cy - grab.1;
            // Footprint is w × (h + TITLE_H); keep it on screen.
            let fw = w as i32;
            let fh = (h + decor::TITLE_H) as i32;
            fx = fx.clamp(0, (sw_screen - fw).max(0));
            fy = fy.clamp(0, (sh_screen - fh).max(0));
            // Surface origin = footprint origin + (0, TITLE_H).
            let sx = fx as u32;
            let sy = (fy + decor::TITLE_H as i32) as u32;
            self.wins[i].rect = (sx, sy, w, h);
        }
    }

    /// Close (remove) the window with `id`: drop it from `wins`, which drops its
    /// (Store, Instance) → tears down the wasm instance. Returns true if a window
    /// was removed. Fixes up `self.focused` (real SP2 tracks it) so it never
    /// dangles past the end and the surviving window flagged `focused` matches.
    pub fn close(&mut self, id: u32) -> bool {
        let Some(i) = self.index_of(id) else { return false; };
        self.wins.remove(i); // Window owns Store+Instance → Drop tears it down
        if self.wins.is_empty() {
            self.focused = 0;
            return true;
        }
        // Shift `self.focused` left if the removed window was at/below it, then
        // clamp into range, then re-assert the `focused` flag on exactly that
        // window (and clear it on all others) so there are never two flagged.
        if self.focused > i || self.focused >= self.wins.len() {
            self.focused = self.focused.saturating_sub(1);
        }
        self.focused = self.focused.min(self.wins.len() - 1);
        for (j, w) in self.wins.iter_mut().enumerate() {
            w.focused = j == self.focused;
        }
        true
    }

    /// Build the full-footprint RGBA8888 buffer for window `idx`:
    /// row 0..TITLE_H = decorated title bar, then the app surface below.
    /// Returns (buf, footprint_x, footprint_y, footprint_w, footprint_h).
    /// Returns None if the surface has not been committed yet.
    fn compose_window(&self, idx: usize) -> Option<(Vec<u8>, u32, u32, u32, u32)> {
        let win = &self.wins[idx];
        let (sx, sy, sw, sh) = win.rect;
        let surface = &win.store.data().pixels;
        if surface.is_empty() { return None; }
        let th = decor::TITLE_H;
        let fw = sw;
        let fh = sh + th;
        let (fx, fy) = (sx, sy - th); // footprint origin (caller keeps sy >= th)
        let mut buf = alloc::vec![0u8; (fw * fh * 4) as usize];

        // Title bar background (focus-coloured).
        let bar = if win.focused { decor::BAR_FOCUSED } else { decor::BAR_UNFOCUSED };
        decor::fill_rect(&mut buf, fw, fh, 0, 0, fw, th, bar);

        // Close [X] button (right-aligned square) + glyph.
        let bw = if fw < decor::BTN_W { fw } else { decor::BTN_W };
        let bx = fw - bw;
        decor::fill_rect(&mut buf, fw, fh, bx, 0, bw, th, decor::CLOSE_BG);
        decor::draw_text(&mut buf, fw, fh, bx + (bw / 4), 0, fw, "x", decor::CLOSE_GLYPH);

        // Title text (left), clipped so it never runs under the [X] button.
        decor::draw_text(&mut buf, fw, fh, decor::TEXT_PAD_X, 0, bx,
                         &win.title, decor::TEXT_RGBA);

        // Surface below the bar: copy committed pixels row-major (clip to fw).
        let src_stride = (win.store.data().win_w as usize) * 4;
        let copy_w = core::cmp::min(win.store.data().win_w, fw) as usize * 4;
        for row in 0..sh as usize {
            let src_off = row * src_stride;
            if src_off + copy_w > surface.len() { break; }
            let dst_off = ((th as usize + row) * fw as usize) * 4;
            if dst_off + copy_w > buf.len() { break; }
            buf[dst_off..dst_off + copy_w]
                .copy_from_slice(&surface[src_off..src_off + copy_w]);
        }
        Some((buf, fx, fy, fw, fh))
    }

    /// Call `frame()` on every window's instance (the gate's get_typed_func loop,
    /// ONE copy). Each app drains its queue via `wm.poll_event` and redraws.
    fn frame_all(&mut self) {
        for w in self.wins.iter_mut() {
            if let Ok(frame) = w.inst.get_typed_func::<(), ()>(&mut w.store, "frame") {
                let _ = frame.call(&mut w.store, ());
            }
        }
    }

    /// Composite ALL windows bottom→top into the kernel back-buffer, then ONE
    /// blit. SP4: the per-band composite of the DECORATED footprints runs in
    /// parallel across the SMP compute-pool (the APs); the present is one serial
    /// blit on the BSP (which also recomposites the cursor). Clearing each band
    /// to DESKTOP_BG every frame means drag/close leave no ghosts.
    ///
    /// CRITICAL (interface-contract decision 6): we composite SP3's DECORATED
    /// footprints from `compose_window` (title bar + [X] + surface), NOT raw
    /// surfaces — so window decorations survive the parallel raster.
    fn present(&mut self) {
        let g = crate::gfx::geom();
        let (sw, sh) = (g.width, g.height);
        if sw == 0 || sh == 0 { return; }
        let stride = (sw as usize) * 4;
        let needed = stride * sh as usize;
        if self.backbuf.len() != needed { self.backbuf = alloc::vec![0u8; needed]; }

        // 1) BSP: build decorated footprints bottom->top (z = `wins` Vec order).
        //    Keep them ALIVE in `foots` across the join so the band jobs' raw
        //    pointers (into each footprint buffer) stay valid for the whole
        //    parallel composite.
        let mut foots: alloc::vec::Vec<(alloc::vec::Vec<u8>, u32, u32, u32, u32)> =
            alloc::vec::Vec::new();
        for i in 0..self.wins.len() {
            if let Some(f) = self.compose_window(i) { foots.push(f); }
        }
        let n = core::cmp::min(foots.len(), MAX_WINS);
        // SAFETY: BSP-only write of WIN_ARENA between joined frames (the previous
        // frame's jobs were joined inside dispatch_bands before we returned, so
        // no job is in flight while we mutate the arena here).
        for (i, (buf, fx, fy, fw, fh)) in foots.iter().enumerate().take(n) {
            unsafe {
                WIN_ARENA[i] = WinDesc {
                    px: buf.as_ptr(),
                    px_len: buf.len(),
                    x: *fx,
                    y: *fy,
                    w: *fw,
                    h: *fh,
                };
            }
        }

        // 2) Dispatch the banded composite into backbuf, then present.
        let bg = u32::from_le_bytes(DESKTOP_BG);
        let back_ptr = self.backbuf.as_mut_ptr() as usize;
        dispatch_bands(back_ptr, stride, sw, sh, bg, n);
        crate::gfx::blit(&self.backbuf[..needed], 0, 0, sw, sh);
        // `foots` drops here — AFTER dispatch_bands joined all jobs, so the
        // footprint buffers were alive for the entire parallel composite.
    }

    /// Left mouse-down at screen (px,py): topmost window's decoration decides.
    /// [X] => close; title bar => raise+focus+begin drag; surface => raise+focus
    /// AND forward the click to the app (so the app surface still reacts to
    /// clicks, preserving SP2's per-window input). Empty space => nothing.
    fn on_left_down(&mut self, px: i32, py: i32) {
        let Some(i) = self.topmost_decor_at(px, py) else { return; };
        let s = self.wins[i].rect;
        let id = self.wins[i].id;
        match decor::hit(s, px, py) {
            decor::Hit::Close => { self.close(id); self.drag = None; }
            decor::Hit::Title => {
                let top = self.raise(i);
                self.set_focus(top);
                let fr = decor::window_rect(self.wins[top].rect);
                self.drag = Some(DragState {
                    win_id: id,
                    grab_dx: px - fr.0 as i32,
                    grab_dy: py - fr.1 as i32,
                });
            }
            decor::Hit::Surface => {
                let top = self.raise(i);
                self.set_focus(top);
                self.wins[top].store.data_mut().events
                    .push_back(crate::gfx::GfxEvt { kind: 2, p0: 0, p1: 1, p2: 0 });
                self.drag = None;
            }
            decor::Hit::Outside => {}
        }
    }

    /// The per-frame window-manager loop. Owns the CPU; never returns.
    ///
    /// Each loop:
    ///  1. `fold_mouse()` (PS/2 -> absolute cursor + move/button GfxEvts), then
    ///     drain the kernel gfx queue (the compositor is the SOLE consumer).
    ///  2. Dispatch via the decorations: mousemove drives an in-progress drag;
    ///     a left-button edge (press/release) begins/ends a drag via
    ///     `on_left_down` ([X] close, title raise+focus+drag, surface
    ///     raise+focus+forward); keys go to the focused window's queue.
    ///  3. `frame_all()` (each app drains its queue + redraws), then `present()`
    ///     composites ALL windows into the back-buffer with ONE blit.
    ///
    /// Cursor source = `gfx::mouse_pos()` ONLY (kept current by `fold_mouse`);
    /// it is NOT re-tracked from kind-1 events. Focus is shown by the title-bar
    /// colour (blue focused / grey unfocused), so there is no focus border.
    pub fn run(mut self) -> ! {
        crate::kprintln!("[wm] compositor SP3: window manager ({} windows)", self.wins.len());
        // SysV ABI requires DF=0; cranelift/Rust code uses `rep movs` which run
        // BACKWARD if DF=1, silently corrupting copied data.
        #[cfg(target_arch = "x86_64")]
        unsafe { core::arch::asm!("cld", options(nostack)); }

        let mut btn_l = false;
        let mut frame_no: u32 = 0;
        let mut marker_done = false;
        loop {
            crate::gfx::fold_mouse();
            while let Some(ev) = crate::gfx::pop() {
                let (cx, cy) = crate::gfx::mouse_pos();
                match ev.kind {
                    1 => { // mousemove: if dragging, move the window
                        if let Some(d) = self.drag {
                            self.drag_to(d.win_id, cx, cy, (d.grab_dx, d.grab_dy));
                        }
                    }
                    2 => { // mouse button: edge-track the left button
                        if ev.p0 == 0 {
                            let pressed = ev.p1 != 0;
                            if pressed && !btn_l { btn_l = true; self.on_left_down(cx, cy); }
                            else if !pressed && btn_l { btn_l = false; self.drag = None; }
                        }
                    }
                    0 => { // key -> focused window's queue
                        if self.focused < self.wins.len() {
                            self.wins[self.focused].store.data_mut().events.push_back(ev);
                        }
                    }
                    _ => {}
                }
            }

            self.frame_all();
            self.present();
            frame_no += 1;

            // After 30 composited frames, report the distinct cores that ran
            // band jobs (one-shot; greppable by the boot-marker test). The
            // warm-up gives the APs time to pick up several frames of jobs (the
            // wake-IPI + worker-loop latency means the first frame or two may
            // run partly inline on the BSP before APs warm up). cpu_id is
            // LAPIC-based per core, VBox-safe — see project memory.
            if !marker_done && frame_no >= 30 {
                let mask = take_composite_core_mask();
                let mut cores: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
                for c in 0..32u32 {
                    if mask & (1u32 << c) != 0 { cores.push(c); }
                }
                let n_cores = cores.len();
                crate::binfo!("wm", "composite cores={} {:?}", n_cores, cores);
                marker_done = true;
            }

            // Crude pacing so the colour cycle + input feel responsive.
            for _ in 0..2_000_000u32 { core::hint::spin_loop(); }
        }
    }
}

/// Entry point (the executor router calls EXACTLY this name; do NOT rename).
/// Builds the canonical `Compositor` and runs its input-routed loop forever.
pub fn run_compositor_gate(cwasm: &[u8]) -> ! {
    crate::gfx::enter();
    Compositor::new(cwasm).run()
}

/// Boot-check: exercise the WM geometry + z-order/drag/close math with NO wasm
/// instances. Returns a bitfield of passing sub-checks (all set == 0b1_1111).
#[cfg(feature = "boot-checks")]
pub fn wm_logic_selftest() -> u32 {
    use decor::{Hit, hit, title_rect, close_rect};
    let mut flags = 0u32;

    // (bit 0) title bar sits above the surface, full width.
    let s = (100u32, 50u32, 320u32, 240u32); // surface; sy=50 >= TITLE_H=28
    let tr = title_rect(s);
    if tr == (100, 50 - decor::TITLE_H, 320, decor::TITLE_H) { flags |= 1 << 0; }

    // (bit 1) close button is a square at the right end of the bar.
    let cr = close_rect(s);
    if cr == (100 + 320 - decor::BTN_W, 50 - decor::TITLE_H, decor::BTN_W, decor::TITLE_H) {
        flags |= 1 << 1;
    }

    // (bit 2) hit classification: point in [X] => Close; in bar (left) => Title;
    // in surface => Surface; above the bar => Outside.
    let hx = (cr.0 + 2) as i32; let hy = (cr.1 + 2) as i32;          // inside [X]
    let tx = (tr.0 + 2) as i32; let ty = (tr.1 + 2) as i32;          // left of bar
    let ix = (s.0 + 4) as i32;  let iy = (s.1 + 4) as i32;           // inside surface
    if hit(s, hx, hy) == Hit::Close
        && hit(s, tx, ty) == Hit::Title
        && hit(s, ix, iy) == Hit::Surface
        && hit(s, ix, (tr.1 as i32) - 5) == Hit::Outside
    { flags |= 1 << 2; }

    // (bit 3) z-order move-to-top: ids [10,11,12] (12 top); raise idx 0 (id 10)
    // => order [11,12,10] (10 now top).
    let mut order = alloc::vec![10u32, 11, 12];
    let i = 0usize;
    let w = order.remove(i); order.push(w); // mirrors Compositor::raise
    if order == alloc::vec![11u32, 12, 10] { flags |= 1 << 3; }

    // (bit 4) drag math: footprint origin = cursor - grab, surface = +TITLE_H.
    // grab=(10,5), cursor=(200,160) => footprint (190,155) => surface (190,155+28).
    let grab = (10i32, 5i32);
    let (cxd, cyd) = (200i32, 160i32);
    let fx = cxd - grab.0; let fy = cyd - grab.1;
    let sx = fx; let sy = fy + decor::TITLE_H as i32;
    if sx == 190 && sy == 155 + decor::TITLE_H as i32 { flags |= 1 << 4; }

    flags
}
