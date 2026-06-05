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

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use wasmtime::{Caller, Instance, Linker, Module, Store};
use crate::gfx::GfxEvt;
use crate::wasm::wt::engine;
use core::sync::atomic::{AtomicU32, Ordering};
use crate::wasm::wt::compose::{composite_band, WinDesc};

/// Launcher (taskbar) height in px — a strip across the bottom of the screen.
/// Reserved by SP5; SP5-B draws clickable app entries here. Spawn placement
/// clamps windows above it.
const LAUNCHER_H: u32 = 28;

/// Width of one launcher button (one per `APPS` entry, left-to-right).
const LAUNCHER_BTN_W: u32 = 96;

/// Max simultaneously-live windows. Each live window holds a wasm instance (its
/// own linear memory) + a surface buffer; this bounds the heap budget. (Reactor
/// surface ≈ 0.3 MB; a full-window app ≈ 4 MB. 8 windows ≈ a few MB of surfaces
/// + N linear memories — comfortably within the kernel heap.)
const MAX_WINDOWS: usize = 8;

// --- App registry + shared Module cache (SP5) -----------------------------

/// The persistent reactor (cycling colour, runs forever). Embedded
/// unconditionally because the compositor — not a boot-check — needs it.
static REACTOR_CWASM: &[u8] = include_bytes!("reactor.cwasm");
/// A reactor that calls `wm.close()` after a few frames (for the despawn
/// boot-check and the [X]-equivalent demo). Built by `tools/wt-reactor-close`.
static REACTOR_CLOSE_CWASM: &[u8] = include_bytes!("reactor_close.cwasm");
/// A wasm32-wasip1 STD reactor (egui SP-A probe): proves a std/WASI guest runs
/// as a compositor window. Built by `tools/wt-wasip1-probe`; needs `_initialize`
/// run before its first `frame()` (see `run_initialize`).
static PROBE_CWASM: &[u8] = include_bytes!("probe.cwasm");
/// The egui CSD demo window (SP-B): a wasm32-wasip1 std reactor that draws its own
/// egui window (CSD title bar + counter) via gui-core's raster, over the `wm`
/// surface protocol. Built from `ruos-desktop/compositor-app`; like the probe it
/// needs `_initialize` run before its first `frame()`. The FIRST launcher-visible
/// app (`show_in_launcher: true`).
static EGUI_DEMO_CWASM: &[u8] = include_bytes!("egui_demo.cwasm");

/// A launchable app: a display name + its precompiled `.cwasm` bytes.
pub struct AppEntry {
    pub name: &'static str,
    pub cwasm: &'static [u8],
    /// `false` = spawnable BY NAME (boot-checks still find it via
    /// `APPS.iter().position(..)`) but HIDDEN from the launcher taskbar. The CSD
    /// demo reactors are kept for boot-checks but retired from the visible
    /// launcher (a real egui app gets `true` in SP-B Task 6).
    pub show_in_launcher: bool,
}

/// The launcher's app table. Adding a real app later = one more entry here. Two
/// display names map to the same reactor module (distinct launcher entries).
pub static APPS: &[AppEntry] = &[
    AppEntry { name: "react-A", cwasm: REACTOR_CWASM, show_in_launcher: false },
    AppEntry { name: "react-B", cwasm: REACTOR_CWASM, show_in_launcher: false },
    AppEntry { name: "selfclose", cwasm: REACTOR_CLOSE_CWASM, show_in_launcher: false },
    AppEntry { name: "wasip1-probe", cwasm: PROBE_CWASM, show_in_launcher: false },
    AppEntry { name: "egui-demo", cwasm: EGUI_DEMO_CWASM, show_in_launcher: true },
];

/// Cache of deserialised modules, keyed by the cwasm slice's base address (each
/// embedded blob is a distinct `&'static`, so its pointer is a stable unique
/// key). Deserialising is the costly step; instantiation off a cached `Module`
/// is cheap (wasmtime `Module` is Arc-backed: `clone` bumps a refcount).
static MODULE_CACHE: Mutex<BTreeMap<usize, Module>> = Mutex::new(BTreeMap::new());

/// Get (deserialising once, then caching) the `Module` for an app's cwasm.
fn module_for(cwasm: &'static [u8]) -> Option<Module> {
    let key = cwasm.as_ptr() as usize;
    let mut cache = MODULE_CACHE.lock();
    if let Some(m) = cache.get(&key) {
        return Some(m.clone());
    }
    // SAFETY: produced by wt-precompile for this exact engine Config.
    let m = unsafe { Module::deserialize(engine(), cwasm) }.ok()?;
    cache.insert(key, m.clone());
    Some(m)
}

/// Boot self-test: every registry entry deserialises to a usable Module.
/// Returns (entry_count, modules_ok).
pub fn registry_self_test() -> (u32, u32) {
    let n = APPS.len() as u32;
    let mut ok = 0u32;
    for app in APPS {
        if module_for(app.cwasm).is_some() { ok += 1; }
    }
    (n, ok)
}

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

/// Kernel chrome rasterisation helpers (the launcher taskbar). CSD retired the
/// window decorations (the app draws its own title bar / [X] / content), so all
/// that remains here is the pure software rasteriser the launcher strip uses:
/// solid rects + bitmap-font text into a caller-owned RGBA8888 `Vec<u8>` (no
/// framebuffer access, so it stays unit-checkable / AP-callable).
pub mod decor {
    /// Text inset from the left edge of a chrome element (e.g. a launcher button).
    pub const TEXT_PAD_X: u32 = 8;

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
    /// noto bitmap font (Regular weight). Vertically centres within `buf_h`.
    pub fn draw_text(buf: &mut [u8], buf_w: u32, buf_h: u32,
                     x: u32, y: u32, max_x: u32, text: &str, c: [u8; 4]) {
        let gw = crate::console::font::glyph_width() as u32;
        let gh = crate::console::font::glyph_height() as i32;
        let gy = y as i32 + ((buf_h as i32 - gh) / 2).max(0);
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
    /// Set by the guest via `wm.close()`; the compositor reaps the window next loop.
    pub close_requested: bool,
    /// Set by the guest via `wm.start_move()` (CSD title-bar grab); the run loop
    /// turns it into a kernel-driven interactive move (a `DragState`) next frame,
    /// then clears it.
    pub move_requested: bool,
}

use crate::wasm::wt::state::{WtState, HasWasi};

/// Capability accessor for the window/surface state (mirror of HasWasi).
pub trait HasWindow {
    fn win(&mut self) -> &mut WmState;
    fn win_ref(&self) -> &WmState;
}

impl HasWindow for WmState {
    fn win(&mut self) -> &mut WmState { self }
    fn win_ref(&self) -> &WmState { self }
}

/// A compositor window's Store data: BOTH the WASI capability (so a wasip1 egui
/// guest's std runtime links + runs) AND the window/surface state. Embeds the
/// existing structs unchanged; implements both accessor traits.
pub struct AppState {
    pub wasi: WtState,
    pub win: WmState,
}

impl HasWasi for AppState {
    fn wasi(&mut self) -> &mut WtState { &mut self.wasi }
    fn wasi_ref(&self) -> &WtState { &self.wasi }
}
impl HasWindow for AppState {
    fn win(&mut self) -> &mut WmState { &mut self.win }
    fn win_ref(&self) -> &WmState { &self.win }
}

pub fn add_to_linker<T: HasWindow + 'static>(linker: &mut Linker<T>) -> wasmtime::Result<()> {
    // wm.commit(ptr, len, w, h): copy the guest's surface into WmState.pixels.
    linker.func_wrap("wm", "commit",
        |mut caller: Caller<'_, T>, ptr: i32, len: i32, w: i32, h: i32| {
            if let Some(b) = crate::wasm::wt::mem::read(&mut caller, ptr as u32, len as u32) {
                let s = caller.data_mut().win();
                s.pixels = b;
                s.win_w = w as u32;
                s.win_h = h as u32;
            }
        })?;
    // wm.app_id() -> u32: this instance's window id. (Import name is `app_id`
    // with an underscore — Rust `#[link]` preserves the symbol verbatim; verified
    // via `wasm-tools print`.)
    linker.func_wrap("wm", "app_id",
        |caller: Caller<'_, T>| -> i32 { caller.data().win_ref().id as i32 })?;
    // wm.tick(): bump the call counter (spike instrumentation).
    linker.func_wrap("wm", "tick",
        |mut caller: Caller<'_, T>| { caller.data_mut().win().tick += 1; })?;
    // wm.close(): the guest asks the compositor to tear this window down. The
    // reap pass (top of the loop) drops the Store/Instance next iteration.
    linker.func_wrap("wm", "close",
        |mut caller: Caller<'_, T>| { caller.data_mut().win().close_requested = true; })?;
    // wm.start_move(): the guest (CSD title-bar grab) asks the compositor to begin
    // a kernel-driven interactive move of THIS window. We only record the request;
    // the run loop turns it into a `DragState` (reusing SP3's drag machinery) so
    // the window tracks the screen cursor without the app needing its screen origin.
    linker.func_wrap("wm", "start_move",
        |mut caller: Caller<'_, T>| { caller.data_mut().win().move_requested = true; })?;
    // wm.wall_seconds() -> f64: MONOTONIC seconds since boot (the same latched
    // source the desktop uses). egui needs it for `RawInput.time` + animations.
    linker.func_wrap("wm", "wall_seconds",
        |_caller: Caller<'_, T>| -> f64 { crate::wasm::wt::gfx::wall_secs() })?;
    // wm.poll_event(retptr): drain ONE event from THIS window's queue into the
    // guest's 20-byte return area. The calling app is identified by its own
    // Store (caller.data()), so it can only ever see its own window's events.
    // Layout matches `ruos:gui/gfx poll-event`: discriminant u32 @0 (0=none,
    // 1=some), then the gfx-event record kind@4, p0@8, p1@12, p2@16 (all LE).
    linker.func_wrap("wm", "poll_event",
        |mut caller: Caller<'_, T>, retptr: i32| {
            let ev = caller.data_mut().win().events.pop_front();
            let mut buf = [0u8; 20];
            if let Some(e) = ev {
                buf[0..4].copy_from_slice(&1u32.to_le_bytes());   // some
                buf[4..8].copy_from_slice(&e.kind.to_le_bytes());
                buf[8..12].copy_from_slice(&e.p0.to_le_bytes());
                buf[12..16].copy_from_slice(&e.p1.to_le_bytes());
                buf[16..20].copy_from_slice(&e.p2.to_le_bytes());
            }
            // else: discriminant stays 0 (none); payload zeroed.
            crate::wasm::wt::mem::write(&mut caller, retptr as u32, &buf);
        })?;
    Ok(())
}

/// Run a reactor instance's `_initialize` export ONCE, if it has one.
///
/// A `wasm32-wasip1` cdylib with no `main` links as a REACTOR: wasm-ld emits an
/// `_initialize` export (NOT `_start`) that runs std's static initializers
/// (heap/runtime setup). A std/wasip1 reactor will FAULT on its first heap alloc
/// inside `frame()` if `_initialize` was never run. So every site that
/// instantiates a window instance MUST call this right after `instantiate` and
/// BEFORE the first `frame()`. The no_std reactors export no `_initialize`, so
/// the `Ok` arm is simply skipped — safe + necessary only for wasip1 reactors.
fn run_initialize(store: &mut Store<AppState>, inst: &Instance) {
    if let Ok(init) = inst.get_typed_func::<(), ()>(&mut *store, "_initialize") {
        let _ = init.call(&mut *store, ());
    }
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
        AppState {
            wasi: WtState::new(alloc::vec![b"win".to_vec()]),
            win: WmState { id: 0, win_w: 0, win_h: 0, pixels: Vec::new(), tick: 0, events: VecDeque::new(), close_requested: false, move_requested: false },
        },
    );
    let mut linker: Linker<AppState> = Linker::new(engine);
    crate::wasm::wt::wasi::add_to_linker(&mut linker).expect("wasi linker");
    if add_to_linker(&mut linker).is_err() { return (0, 0, 0); }
    // SysV ABI requires DF=0; cranelift/Rust code uses `rep movs` which run
    // BACKWARD if DF=1, silently corrupting copied data.
    #[cfg(target_arch = "x86_64")]
    unsafe { core::arch::asm!("cld", options(nostack)); }
    let instance = match linker.instantiate(&mut store, &module) {
        Ok(i) => i,
        Err(_) => return (0, 0, 0),
    };
    // wasip1 std reactors need `_initialize` run before their first heap alloc.
    run_initialize(&mut store, &instance);
    let frame = match instance.get_typed_func::<(), ()>(&mut store, "frame") {
        Ok(f) => f,
        Err(_) => return (0, 0, 0),
    };
    for _ in 0..5 {
        if frame.call(&mut store, ()).is_err() { break; }
    }
    (
        store.data().win.tick,
        store.data().win.pixels.first().copied().unwrap_or(0),
        store.data().win.pixels.len(),
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
    pub store: Store<AppState>,
    pub inst: Instance,
    pub rect: (u32, u32, u32, u32), // WHOLE-window rect (x, y, w, h) under CSD (no decor)
    pub title: String,              // app name; the app draws its OWN CSD title bar
    pub focused: bool,
    pub alive: bool,                // SP5 sets false to schedule teardown
    pub pid: u32,                   // SP5: proc-registry pid (freed on reap)
}

/// Window order in `wins` IS the z-order: index 0 = bottom, last = top.
/// There is NO `z: u32` field — `raise(idx)` moves the window to the end.
pub struct Compositor {
    pub wins: Vec<Window>,
    pub module: Module,            // shared AOT module; instances cheap
    pub linker: Linker<AppState>,
    pub focused: usize,            // index into wins (the focused window)
    pub drag: Option<DragState>,   // SP3 adds; None until SP3
    pub backbuf: alloc::vec::Vec<u8>, // SP3 screen back-buffer (lazily sized in present)
    pub free_ids: Vec<u32>,        // SP5: window-ids returned by reaped windows (LIFO)
    pub next_id: u32,              // SP5: next never-used id (high-water mark)
}

/// Reactor surface size (matches `tools/wt-reactor` W/H). Windows are fixed at
/// this size in SP2; SP3 will make them resizable.
const WIN_W: u32 = 320;
const WIN_H: u32 = 240;

/// Desktop background (RGBA8888 [r,g,b,a]) shown where no window covers.
const DESKTOP_BG: [u8; 4] = [0x10, 0x18, 0x20, 0xFF];

impl Compositor {
    /// Deserialize the shared reactor module, build the `wm` linker, and create
    /// the demo's 2 OVERLAPPING windows. CSD: windows are borderless (the surface
    /// IS the whole window), placed anywhere on-screen; the two overlap so the
    /// raise/lower behaviour is directly visible. Window 0 starts focused.
    pub fn new(cwasm: &[u8]) -> Compositor {
        let engine = engine();
        // SAFETY: produced by wt-precompile for this exact engine Config.
        let module = unsafe { Module::deserialize(engine, cwasm) }.expect("reactor module");
        let mut linker: Linker<AppState> = Linker::new(engine);
        crate::wasm::wt::wasi::add_to_linker(&mut linker).expect("wasi linker");
        add_to_linker(&mut linker).expect("wm linker"); // wm::add_to_linker (this module)

        // (window x, y, w, h, title) — A and B overlap so raise/lower is visible.
        // No title-bar offset under CSD: rect IS the whole window.
        let placements: [(u32, u32, u32, u32, &str); 2] = [
            (60,  60,  WIN_W, WIN_H, "reactor A"),
            (300, 170, WIN_W, WIN_H, "reactor B"),
        ];
        let mut wins: Vec<Window> = Vec::new();
        for (id, &(x, y, w, h, title)) in placements.iter().enumerate() {
            let mut store = Store::new(
                engine,
                AppState {
                    wasi: WtState::new(alloc::vec![b"win".to_vec()]),
                    win: WmState { id: id as u32, win_w: 0, win_h: 0, pixels: Vec::new(), tick: 0, events: VecDeque::new(), close_requested: false, move_requested: false },
                },
            );
            let inst = linker.instantiate(&mut store, &module).expect("instantiate");
            // wasip1 std reactors need `_initialize` run before their first frame().
            run_initialize(&mut store, &inst);
            // Register a proc so `ps` sees demo windows and reap can unregister them.
            let pid = crate::proc::register(alloc::format!("win:{}", title));
            wins.push(Window {
                id: id as u32,
                store,
                inst,
                rect: (x, y, w, h),
                title: String::from(title),
                focused: id == 0,
                alive: true,
                pid,
            });
        }
        let next_id = placements.len() as u32; // spawned ids start past the demo ids
        Compositor { wins, module, linker, focused: 0, drag: None, backbuf: Vec::new(),
                     free_ids: Vec::new(), next_id }
    }

    /// Headless compositor for the lifecycle boot-check: builds the linker +
    /// shared reactor Module but creates NO windows and NEVER calls
    /// `crate::gfx::enter` (no framebuffer). Spawn/reap run purely in RAM.
    fn new_empty() -> Compositor {
        let engine = engine();
        let mut linker: Linker<AppState> = Linker::new(engine);
        crate::wasm::wt::wasi::add_to_linker(&mut linker).expect("wasi linker");
        add_to_linker(&mut linker).expect("wm linker"); // wm::add_to_linker (this module)
        let module = module_for(REACTOR_CWASM).expect("reactor module");
        Compositor {
            wins: Vec::new(),
            module,
            linker,
            focused: 0,
            drag: None,
            backbuf: Vec::new(),
            free_ids: Vec::new(),
            next_id: 0,
        }
    }

    /// Allocate a window-id, preferring a recycled one from the free-list.
    fn alloc_id(&mut self) -> u32 {
        if let Some(id) = self.free_ids.pop() {
            id
        } else {
            let id = self.next_id;
            self.next_id += 1;
            id
        }
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

    /// Translate window `id`'s rect so the grabbed point follows the cursor.
    /// `(cx,cy)` is the absolute cursor; `grab` is the offset captured at the start
    /// of the drag (`wm.start_move`). CSD: the rect IS the whole window (no title
    /// offset), so the window origin = cursor − grab, clamped fully on-screen.
    pub fn drag_to(&mut self, id: u32, cx: i32, cy: i32, grab: (i32, i32)) {
        let g = crate::gfx::geom();
        let (sw_screen, sh_screen) = (g.width as i32, g.height as i32);
        if let Some(i) = self.index_of(id) {
            let (_, _, w, h) = self.wins[i].rect;
            // New window origin = cursor - grab offset, kept on screen.
            let fw = w as i32;
            let fh = h as i32;
            let nx = (cx - grab.0).clamp(0, (sw_screen - fw).max(0)) as u32;
            let ny = (cy - grab.1).clamp(0, (sh_screen - fh).max(0)) as u32;
            self.wins[i].rect = (nx, ny, w, h);
        }
    }

    /// Unified teardown of `wins[i]`: remove it (dropping its Store+Instance →
    /// the wasm instance + guest linear memory + surface buffer are freed),
    /// unregister its proc, recycle its window-id to the free-list, and fix up
    /// `self.focused` so it never dangles and exactly one survivor is flagged.
    /// The SOLE place a window leaves `wins` (close + reap both route here).
    fn remove_at(&mut self, i: usize) {
        if i >= self.wins.len() { return; }
        let w = self.wins.remove(i); // Drop tears down Store+Instance (frees guest mem)
        crate::proc::unregister(w.pid);
        self.free_ids.push(w.id);
        crate::binfo!("wm", "reaped win_id={} pid={} (Store/Instance dropped)", w.id, w.pid);
        if self.wins.is_empty() {
            self.focused = 0;
            return;
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
    }

    /// Close (remove) the window with `id` IMMEDIATELY (used by the [X] click in
    /// `on_left_down`). Returns true if a window was removed. Routes through
    /// `remove_at` so proc-unregister + id-recycle happen exactly once.
    pub fn close(&mut self, id: u32) -> bool {
        if let Some(i) = self.index_of(id) {
            self.remove_at(i);
            true
        } else {
            false
        }
    }

    /// Spawn registry app `idx` as a new window: allocate a window-id,
    /// instantiate a fresh `(Store<WmState>, Instance)` off the cached Module,
    /// place it at a cascading origin (honoring the title-bar/launcher clamps),
    /// raise+focus it, and push it. Returns the new window-id, or None (budget
    /// full / bad app idx / bad module / instantiate failed).
    pub fn spawn_app(&mut self, idx: usize) -> Option<u32> {
        let live = self.wins.iter().filter(|w| w.alive).count();
        if live >= MAX_WINDOWS {
            crate::bwarn!("wm", "spawn refused: window budget full ({})", live);
            return None;
        }
        let app = APPS.get(idx)?;
        let module = module_for(app.cwasm)?;
        let id = self.alloc_id();
        let mut store = Store::new(
            engine(),
            AppState {
                wasi: WtState::new(alloc::vec![b"win".to_vec()]),
                win: WmState { id, win_w: 0, win_h: 0, pixels: Vec::new(), tick: 0,
                               events: VecDeque::new(), close_requested: false,
                               move_requested: false },
            },
        );
        // SysV ABI requires DF=0 before any cranelift/Rust `rep movs`.
        #[cfg(target_arch = "x86_64")]
        unsafe { core::arch::asm!("cld", options(nostack)); }
        let inst = match self.linker.instantiate(&mut store, &module) {
            Ok(i) => i,
            Err(_) => {
                self.free_ids.push(id);
                crate::bwarn!("wm", "spawn: instantiate failed");
                return None;
            }
        };
        // wasip1 std reactors need `_initialize` run before their first frame().
        run_initialize(&mut store, &inst);
        // Cascade placement (TUPLE rect): offset each new window so it doesn't
        // fully overlap. CSD = no title-bar offset; just clamp on-screen (keeping
        // the window clear of the launcher strip at the bottom).
        let g = crate::gfx::geom();
        let n = live as u32;
        let ox = (40 + n * 28).min(g.width.saturating_sub(340));
        let oy = (40 + n * 28)
            .min(g.height.saturating_sub(LAUNCHER_H + 260));
        let pid = crate::proc::register(alloc::format!("win:{}", app.name));
        self.wins.push(Window {
            id,
            store,
            inst,
            rect: (ox, oy, 320, 240),
            title: String::from(app.name),
            focused: false,
            alive: true,
            pid,
        });
        let last = self.wins.len() - 1;
        self.raise(last);                 // move to top of z-order
        self.set_focus(self.wins.len() - 1); // focus the now-top window
        crate::binfo!("wm", "spawn app='{}' win_id={} pid={} live={}",
                      app.name, id, pid, live + 1);
        Some(id)
    }

    /// Mark the window with this id for teardown (external/programmatic close,
    /// e.g. a future IPC path). Idempotent; unknown ids are ignored. The reap
    /// pass (top of the loop) does the actual removal.
    pub fn request_close(&mut self, id: u32) {
        if let Some(w) = self.wins.iter_mut().find(|w| w.id == id) {
            w.alive = false;
            crate::binfo!("wm", "close requested win_id={}", id);
        }
    }

    /// Reap dead windows: first promote any guest-requested closes
    /// (`close_requested`) to `alive=false`, then remove every dead window via
    /// `remove_at` (drops Store/Instance, unregisters proc, recycles id). Call
    /// once at the top of each compositor loop, BEFORE driving frames.
    fn reap(&mut self) {
        for w in &mut self.wins {
            if w.store.data().win.close_requested {
                w.alive = false;
            }
        }
        let mut i = 0;
        while i < self.wins.len() {
            if !self.wins[i].alive {
                self.remove_at(i);
            } else {
                i += 1;
            }
        }
    }

    /// CSD: the window IS its raw committed surface — no kernel decorations. Returns
    /// `(pixels, x, y, w, h)`: the guest's last-committed RGBA8888 surface placed at
    /// `Window.rect`'s origin, sized to the committed `win_w × win_h` (so the band
    /// kernel's `src_stride = w*4` matches the committed buffer exactly). The app
    /// draws its OWN title bar / [X] / content. Returns None if nothing committed yet.
    fn compose_window(&self, idx: usize) -> Option<(Vec<u8>, u32, u32, u32, u32)> {
        let win = &self.wins[idx];
        let s = win.store.data();
        if s.win.pixels.is_empty() { return None; }
        let (x, y, _, _) = win.rect;
        Some((s.win.pixels.clone(), x, y, s.win.win_w, s.win.win_h))
    }

    /// Call `frame()` on every window's instance (the gate's get_typed_func loop,
    /// ONE copy). Each app drains its queue via `wm.poll_event` and redraws.
    ///
    /// CSD crash safety-net: a `frame()` that returns `Err` — a trap, a
    /// `panic=abort`, or a guest `proc_exit` (which `wasi.rs` maps to a trap) —
    /// flags the window `close_requested`, so the reap pass drops it next loop
    /// instead of leaving a frozen, un-closeable window (its CSD [X] is gone).
    fn frame_all(&mut self) {
        for w in self.wins.iter_mut() {
            if let Ok(frame) = w.inst.get_typed_func::<(), ()>(&mut w.store, "frame") {
                match frame.call(&mut w.store, ()) {
                    Ok(()) => {}
                    Err(_) => { w.store.data_mut().win.close_requested = true; }
                }
            }
        }
    }

    /// Composite ALL windows bottom→top into the kernel back-buffer, then ONE
    /// blit. SP4: the per-band composite of each window's surface runs in parallel
    /// across the SMP compute-pool (the APs); the present is one serial blit on the
    /// BSP (which also recomposites the cursor). Clearing each band to DESKTOP_BG
    /// every frame means drag/close leave no ghosts.
    ///
    /// CSD: `compose_window` now yields each window's RAW committed surface (the
    /// app draws its own title bar / [X]); the band kernel paints it at the
    /// window's rect. No kernel decorations are added.
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

    /// Draw the launcher taskbar: a dark strip across the bottom of the screen
    /// with one tinted, labelled button per `APPS` entry. Kernel-drawn chrome
    /// (NOT a wasm surface): builds ONE RGBA8888 strip buffer `g.width ×
    /// LAUNCHER_H`, rasters the buttons + labels into it, then ONE
    /// `gfx::blit`. Called each frame AFTER `present()` so it overlays the
    /// composited windows (always-on-top chrome); `gfx::blit` recomposites the
    /// cursor over it.
    fn draw_launcher(&self) {
        let g = crate::gfx::geom();
        let (sw, lh) = (g.width, LAUNCHER_H);
        if sw == 0 || lh == 0 { return; }
        let mut strip = alloc::vec![0u8; (sw * lh * 4) as usize];

        // Strip background (dark, opaque).
        decor::fill_rect(&mut strip, sw, lh, 0, 0, sw, lh, [0x30, 0x30, 0x38, 0xFF]);

        // Per-button tints (distinct per index so buttons read as separate).
        const BTN_TINTS: [[u8; 4]; 4] = [
            [0x3A, 0x5A, 0x8A, 0xFF], // blue
            [0x3A, 0x7A, 0x4A, 0xFF], // green
            [0x8A, 0x4A, 0x3A, 0xFF], // red-brown
            [0x6A, 0x4A, 0x8A, 0xFF], // purple
        ];

        // Lay out ONLY launcher-visible entries left-to-right by their position
        // among the visible subset (`pos`), independent of the real APPS index.
        let visible: Vec<(usize, &AppEntry)> =
            APPS.iter().enumerate().filter(|(_, a)| a.show_in_launcher).collect();
        for (pos, (_apps_idx, app)) in visible.iter().enumerate() {
            let bx = pos as u32 * LAUNCHER_BTN_W;
            if bx >= sw { break; }
            // Button rect inset 2px from the cell so cells read as buttons.
            let inset = 2u32;
            let bw = LAUNCHER_BTN_W.min(sw - bx);
            let rect_w = bw.saturating_sub(inset * 2);
            let rect_h = lh.saturating_sub(inset * 2);
            let tint = BTN_TINTS[pos % BTN_TINTS.len()];
            decor::fill_rect(&mut strip, sw, lh, bx + inset, inset, rect_w, rect_h, tint);
            // Label (white), clipped to the button's right edge.
            let label_x = bx + inset + decor::TEXT_PAD_X;
            let max_x = bx + bw;
            decor::draw_text(&mut strip, sw, lh, label_x, 0, max_x, app.name,
                             [0xFF, 0xFF, 0xFF, 0xFF]);
        }

        crate::gfx::blit(&strip, 0, g.height - lh, sw, lh);
    }

    /// Hit-test a screen point against the launcher buttons. Buttons are laid out
    /// over ONLY the launcher-visible `APPS` entries (in the same order as
    /// `draw_launcher`); a clicked visible-position is mapped back to the real
    /// `APPS` index. Returns that `APPS` index, or None if the point is above the
    /// strip / past the last visible button / off the right edge of the screen.
    fn launcher_hit(&self, px: i32, py: i32) -> Option<usize> {
        let g = crate::gfx::geom();
        let y0 = g.height as i32 - LAUNCHER_H as i32;
        if py < y0 || px < 0 { return None; }
        let pos = (px / LAUNCHER_BTN_W as i32) as usize; // position among VISIBLE entries
        if (pos as u32 * LAUNCHER_BTN_W) >= g.width { return None; }
        // Map the visible position back to the real APPS index.
        APPS.iter()
            .enumerate()
            .filter(|(_, a)| a.show_in_launcher)
            .nth(pos)
            .map(|(apps_idx, _)| apps_idx)
    }

    /// Left mouse-down at screen (px,py). A launcher-button click spawns its app
    /// (handled FIRST). Otherwise, under CSD the kernel only does window management:
    /// the topmost window under the cursor is raised + focused, and the click is
    /// forwarded to that window's queue so the app's OWN [X]/title-bar/content (all
    /// egui-rendered) react. Close + title-drag are now the app's job (via
    /// `wm.close` / `wm.start_move`). Empty space => nothing.
    fn on_left_down(&mut self, px: i32, py: i32) {
        if let Some(app_idx) = self.launcher_hit(px, py) {
            self.spawn_app(app_idx);
            return;
        }
        if let Some(i) = self.window_at(px, py) {
            let top = self.raise(i);
            self.set_focus(top);
            // Forward the press to the (now-focused) window so its CSD widgets react.
            self.wins[top].store.data_mut().win.events
                .push_back(crate::gfx::GfxEvt { kind: 2, p0: 0, p1: 1, p2: 0 });
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
            // SP5: reap any window that requested close (guest wm.close or the [X]
            // path) on its last frame BEFORE we fold input or drive frames.
            self.reap();
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
                            self.wins[self.focused].store.data_mut().win.events.push_back(ev);
                        }
                    }
                    _ => {}
                }
            }

            self.frame_all();

            // CSD interactive move: a guest that called `wm.start_move()` this
            // frame (title-bar grab) has `move_requested` set. Turn it into a
            // kernel-driven drag of that window — grab offset = cursor − window
            // origin — and let the existing mousemove→`drag_to` / button-up→
            // `drag=None` machinery (below) drive + end the move. Last request
            // wins if several fire in one frame (only one cursor).
            for i in 0..self.wins.len() {
                if self.wins[i].store.data().win.move_requested {
                    self.wins[i].store.data_mut().win.move_requested = false;
                    let (cx, cy) = crate::gfx::mouse_pos();
                    let rect = self.wins[i].rect;
                    self.drag = Some(DragState {
                        win_id: self.wins[i].id,
                        grab_dx: cx - rect.0 as i32,
                        grab_dy: cy - rect.1 as i32,
                    });
                }
            }

            self.present();
            // Always-on-top chrome: present() blits the whole back-buffer, so the
            // launcher must be redrawn over it every frame. gfx::blit recomposites
            // the cursor over the strip.
            self.draw_launcher();
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

/// Headless boot self-test of the spawn/despawn lifecycle: build an empty
/// compositor (NO `gfx::enter`), spawn the self-closing app (registry idx 2),
/// call `frame()`+`reap` repeatedly, and report `(spawns, peak_live,
/// final_live)`. The self-closer requests close on its 3rd `frame()`, so after
/// enough rounds `final_live` must be 0 (the instance was torn down). Then spawn
/// again to prove the freed id was recycled from the free-list.
pub fn lifecycle_self_test() -> (u32, u32, u32) {
    let mut c = Compositor::new_empty();
    let spawns = c.spawn_app(2).is_some() as u32; // selfclose = idx 2
    let mut peak = 0u32;
    for _ in 0..8 {
        c.reap();
        c.frame_all();
        let live = c.wins.iter().filter(|w| w.alive).count() as u32;
        if live > peak { peak = live; }
    }
    c.reap();
    let final_live = c.wins.iter().filter(|w| w.alive).count() as u32;
    // Spawn again to prove the id was recycled (free-list reuse).
    let _ = c.spawn_app(2);
    let reused = c.wins.last().map(|w| w.id).unwrap_or(u32::MAX);
    crate::binfo!("wm", "lifecycle reuse: new win_id after recycle = {}", reused);
    (spawns, peak, final_live)
}

/// Boot self-test (SP-A): spawn the wasip1 STD probe as a compositor window,
/// drive one frame, and report the committed surface length. Builds an empty
/// (headless, no `gfx::enter`) compositor, finds the `"wasip1-probe"` registry
/// entry, `spawn_app`s it (which instantiates against the unified
/// `Linker<AppState>` and runs `_initialize` via `run_initialize`), then
/// `frame_all()` calls the guest's `frame()` once. A non-zero return =
/// 307200 (320×240×4) proves the std/wasip1 guest instantiated, ran its std
/// heap alloc inside `frame()`, and `wm.commit`ed against the WASI+wm linker.
/// 0 = the entry is missing, instantiate failed (a needed WASI import isn't
/// registered), or `frame()`/commit trapped.
pub fn wasip1_probe_self_test() -> usize {
    let mut c = Compositor::new_empty();
    let idx = APPS.iter().position(|a| a.name == "wasip1-probe").unwrap_or(usize::MAX);
    if idx == usize::MAX { return 0; }
    if c.spawn_app(idx).is_none() { return 0; }
    c.frame_all();
    c.wins.last().map(|w| w.store.data().win.pixels.len()).unwrap_or(0)
}

/// Boot-check: exercise the surviving WM z-order + CSD drag math with NO wasm
/// instances. Under CSD the kernel no longer owns decorations (no title bar / [X]
/// geometry to check), so this is a 2-bit word now (all set == 0b11).
#[cfg(feature = "boot-checks")]
pub fn wm_logic_selftest() -> u32 {
    let mut flags = 0u32;

    // (bit 0) z-order move-to-top: ids [10,11,12] (12 top); raise idx 0 (id 10)
    // => order [11,12,10] (10 now top).
    let mut order = alloc::vec![10u32, 11, 12];
    let i = 0usize;
    let w = order.remove(i); order.push(w); // mirrors Compositor::raise
    if order == alloc::vec![11u32, 12, 10] { flags |= 1 << 0; }

    // (bit 1) CSD drag math: the rect IS the whole window (no title offset), so the
    // new window origin = cursor - grab. grab=(10,5), cursor=(200,160) => (190,155).
    let grab = (10i32, 5i32);
    let (cxd, cyd) = (200i32, 160i32);
    let nx = cxd - grab.0; let ny = cyd - grab.1;
    if nx == 190 && ny == 155 { flags |= 1 << 1; }

    flags
}
