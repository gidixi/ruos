> **⚠️ READ THE INTERFACE CONTRACT FIRST:** `2026-06-05-compositor-subprojects-interface-contract.md` — AUTHORITATIVE. Parallelize **SP3's `Compositor::present()`** by banding the kernel back-buffer across the AP pool, compositing SP3's **DECORATED footprints** (`compose_window`), NOT raw surfaces (decorations must survive). `Window` has **NO `pixels`/`z` fields**: read pixels via `w.store.data().pixels`; z-order = `wins` Vec order (no `sort_by_key(z)`). The serial-reference build must keep SP2's `poll_event` registered in the linker (the default `reactor.cwasm` imports it). Grep `kernel/src/smp` for the real job-dispatch API.

# Compositor SP4 — SMP-Parallel Compositing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Parallelize the per-frame compositing of N windows across the SMP compute-offload pool (the APs), so the pure-CPU pixel work (clear background + blit each window's committed surface into the output) runs on multiple cores while the app `frame()` scheduling stays cooperative on the BSP. The result must be **pixel-identical** to today's serial composite (SP3), and a boot marker must prove the work ran on ≥2 distinct cores.

**Architecture:** The compositor keeps composing into an **offscreen back-buffer** (a screen-sized RGBX `Vec<u8>` owned by the kernel), then presents it to the real Limine framebuffer with one serial copy. Today (SP3) the BSP fills that back-buffer serially: clear + for each window in z-order, blit its surface rect. SP4 splits the back-buffer into **disjoint horizontal bands** (one band per AP-ish), encodes each band as a pure-CPU **job descriptor**, and dispatches the jobs to the existing SMP pool (`crate::smp::pool::submit`). Each job composites only the windows' pixels that fall in *its* band, writing **only** into that band's rows of the back-buffer — disjoint output regions ⇒ no data races. The BSP joins all jobs (`poll_done`), then does the single serial present (back-buffer → framebuffer via the existing `gfx` fast-path). App `frame()` calls remain serial on the BSP; only the raster goes parallel.

The hard constraint the pool imposes: `submit(work: fn(&[u8]) -> u64, input: &'static [u8])`. A job is a **plain `fn` pointer** (no captures), receives a `'static [u8]`, and returns a `u64`. It cannot borrow the window list or the back-buffer. SP4 threads everything through that single `&[u8]` by encoding a **band descriptor** (raw pointers + dimensions as little-endian bytes) into a `'static` scratch arena, exactly mirroring how `pool::run_slot` already reconstructs a `&[u8]` from a raw `ptr/len`. The job reconstructs the back-buffer pointer + the window descriptors from those bytes and writes its band with `unsafe` raw stores. The arena is a real `static` (so the `'static` bound is sound); the BSP blocks on join before reusing it, so no two frames alias it.

**Tech Stack:** Rust pinned nightly (`nightly-2026-05-26`), kernel `no_std`, target `x86_64-unknown-none`, build-std via WSL. wasmtime 45 core `Module`/`Instance` (persistent reactor instances, unchanged from SP3). SMP compute pool = `crate::smp::pool` (`submit`/`take`/`run_slot`/`poll_done`, `JobFn = fn(&[u8]) -> u64`). Framebuffer + present via `crate::gfx`. Verification = boot-check serial markers (`binfo!`) for the mechanism (core accounting) + QEMU+KVM QMP screendump (visual, identical to serial).

---

## Assumed interfaces from prior sub-projects (SP2 input/focus, SP3 window manager)

SP4 depends on SP3 (window manager) being COMPLETE and merged. SP2/SP3 plans are not written yet; SP4 assumes these **concrete** signatures and shapes. If SP3 lands with different names, the implementer renames at the call sites flagged below — the algorithm is unchanged.

- **SP3 window model** lives in `kernel/src/wasm/wt/wm.rs`, extending the gate's `WmState`. SP4 assumes a per-window record:
  ```rust
  /// One composited window (SP3). `rect` is the on-screen placement; `pixels`
  /// is the app's last committed RGBA8888 surface (win_w × win_h × 4).
  pub struct Window {
      pub id: u32,
      pub x: u32, pub y: u32,          // top-left on screen (post-drag, SP3)
      pub win_w: u32, pub win_h: u32,  // surface dims
      pub pixels: alloc::vec::Vec<u8>, // committed surface (RGBA8888)
      pub z: u32,                      // z-order: higher = nearer front
      pub focused: bool,               // SP2/SP3 focus flag
  }
  ```
  (The gate's `WmState { id, win_w, win_h, pixels, tick }` is the wasmtime *store data*; SP3 keeps that as store data AND maintains a separate `Window` list for placement/z-order. SP4 reads the `Window` list to composite. If SP3 instead stores placement *inside* `WmState`, adapt the field reads in Task 2 Step 2.)

- **SP3 compositor entry** is a function in `wm.rs` that owns the framebuffer and runs the reactor loop, analogous to the gate's `run_compositor_gate(cwasm: &[u8]) -> !`. SP4 assumes SP3 has refactored the per-frame composite into a dedicated function:
  ```rust
  /// SP3: compose all windows in z-order into the framebuffer for one frame.
  /// Today: serial — clear + blit each window's surface to its rect.
  fn present_frame(windows: &[Window]);
  ```
  SP4 **replaces the body** of `present_frame` (or adds a parallel variant `present_frame_smp`) with the banded parallel path. The reactor loop (call each `frame()`, copy committed surface into the `Window.pixels`) is untouched and stays on the BSP.

- **SP3 z-order** is total and ascending in `z`: windows are composited back-to-front (lowest `z` first). SP4 sorts the window list by `z` once per frame and feeds that order to every band job (painter's algorithm within each band).

- **Framebuffer geometry** from the gate / `crate::gfx`:
  - `crate::gfx::geom() -> GfxGeom { width: u32, height: u32, stride: u32, format: u32 }` (stride = pitch in bytes; format 0 = RGBA8888 canonical).
  - `crate::gfx::blit(buf: &[u8], x: u32, y: u32, w: u32, h: u32)` blits an RGBA8888 rect to the framebuffer, clips to screen, recomposites the software cursor. SP4 uses **one** `blit` of the whole back-buffer per frame to present (so the cursor recompositing path is preserved and the framebuffer is written serially by the BSP only).
  - `crate::gfx::enter()` / `crate::gfx::leave()` GUI-mode toggles (already called by the SP3 entry).

- **SMP pool** (verified, `kernel/src/smp/pool.rs`):
  - `pub type JobFn = fn(&[u8]) -> u64;`
  - `pub fn submit(work: JobFn, input: &'static [u8]) -> Option<usize>;` — returns slot id, `None` if full (`MAX_JOBS = 64`). Sends a wake IPI to APs.
  - `pub fn take() -> Option<usize>;` / `pub fn run_slot(id: usize, cpu: u32);` — used by the BSP to drain inline when no APs exist (1-CPU fallback).
  - `pub fn poll_done(id: usize) -> Option<(u64, u32)>;` — `(result, ran_on_cpu)`; frees the slot.
  - `pub fn is_empty() -> bool;`
- **Core accounting:** `crate::cpu::cpus_online() -> u32`, `crate::cpu::cpu_id() -> u32` (LAPIC-based — never `gs:[0]`, per the VBox quirk in project memory).

---

## File Structure
- `kernel/src/wasm/wt/wm.rs` — **Modify.** Add the parallel-composite module: a `static` job arena, the band `JobFn`, the `composite_parallel(windows, &mut backbuf)` dispatcher, and rewire SP3's `present_frame` to call it. Add a boot-check marker recording the distinct cores that ran composite jobs.
- `kernel/src/wasm/wt/compose.rs` — **Create.** The pure pixel kernel shared by the serial and parallel paths: `fn composite_band(backbuf_ptr, backbuf_stride, band_y0, band_y1, screen_w, &[WinDesc])`. Pure, no kernel deps, so it is identical whether run on the BSP or an AP (the parallel jobs call it; the 1-CPU fallback calls it; the serial reference path calls it).
- `tests/comp-smp-test.sh` — **Create.** Boots `-smp 4` with the compositor init, captures the boot marker over serial, asserts ≥2 distinct cores ran the composite + a screendump matches the serial reference.
- `Makefile` — **Modify.** Add a `run-comp-smp-test` target wiring the new script (build the comptest ISO with the compositor init, run the screendump + marker assertions). Reuse `build/shot.py`.
- `CHANGELOG/NN-26-06-05-compositor-sp4-smp-compositing.md` — **Create** (next free `NN`, currently `278`).

---

## Task 1: The pure pixel kernel (shared, deterministic, core-agnostic)

Extract the per-band composite into a pure function with **no** kernel/global state, so the same code runs on the BSP (serial reference + 1-CPU fallback) and on an AP (parallel). This guarantees the parallel result is byte-identical to serial: same function, same inputs, only the *band range* differs per job.

**Files:** Create `kernel/src/wasm/wt/compose.rs`; add `pub mod compose;` to `kernel/src/wasm/wt/mod.rs`.

- [ ] **Step 1: Window descriptor + band kernel.** Create `kernel/src/wasm/wt/compose.rs`:
```rust
//! Pure compositing kernel shared by the serial reference path and the
//! SMP-parallel band jobs. NO kernel globals, NO I/O, NO allocation — a band
//! job runs this on an AP, the 1-CPU fallback runs it on the BSP, and the
//! serial reference runs it on the BSP; identical inputs ⇒ identical output.
//!
//! The back-buffer is RGBX8888, row-major, `stride` BYTES per row (>= w*4).
//! Compositing is painter's algorithm: callers pass windows already sorted
//! back-to-front (ascending z); each window is opaque (alpha ignored — the
//! gate/SP3 surfaces are solid RGBA, last writer wins per pixel).

/// One window as the band kernel sees it: a raw pointer to its committed
/// RGBA8888 surface plus its on-screen rect. `'static`-free; the dispatcher
/// guarantees the pixels outlive the job (BSP blocks on join before freeing).
#[derive(Copy, Clone)]
pub struct WinDesc {
    pub px: *const u8, // window surface base (RGBA8888, src_stride = win_w*4)
    pub px_len: usize, // surface length in bytes (bounds guard)
    pub x: u32,        // on-screen top-left x
    pub y: u32,        // on-screen top-left y
    pub w: u32,        // surface width  (px)
    pub h: u32,        // surface height (px)
}

// SAFETY: WinDesc only carries a raw pointer + plain integers. It is read-only
// in the band kernel and the dispatcher keeps the pointee alive across the
// join. We send descriptors to AP cores, so assert the marker traits.
unsafe impl Send for WinDesc {}
unsafe impl Sync for WinDesc {}

/// Composite `wins` (back-to-front) into rows `[band_y0, band_y1)` of a
/// screen-sized RGBX back-buffer.
///
/// `back` points at back-buffer row 0; `stride` is its byte pitch; the buffer
/// is `screen_w` px wide and at least `band_y1` rows tall. Every write lands in
/// `[band_y0, band_y1)` ⇒ two bands with disjoint ranges never alias, so two
/// jobs may run this concurrently on different cores with no synchronization.
///
/// SAFETY: caller guarantees `back .. back + band_y1*stride` is a valid,
/// uniquely-owned (for this band's rows) writable region, and each
/// `WinDesc.px .. px+px_len` is a valid readable surface.
pub unsafe fn composite_band(
    back: *mut u8,
    stride: usize,
    screen_w: u32,
    band_y0: u32,
    band_y1: u32,
    bg: u32, // background RGBX (little-endian u32) for uncovered pixels
    wins: &[WinDesc],
) {
    let sw = screen_w as usize;
    // 1) Clear this band to the background colour.
    let mut row = band_y0 as usize;
    while row < band_y1 as usize {
        let row_ptr = back.add(row * stride) as *mut u32;
        let mut col = 0usize;
        while col < sw {
            // SAFETY: col < screen_w and row < band_y1 ⇒ within the band.
            *row_ptr.add(col) = bg;
            col += 1;
        }
        row += 1;
    }
    // 2) Painter's algorithm: blit each window's overlap with this band.
    for win in wins {
        let wx = win.x as usize;
        let wy = win.y as usize;
        let ww = win.w as usize;
        let wh = win.h as usize;
        if ww == 0 || wh == 0 { continue; }
        let src_stride = ww * 4; // surface is RGBA8888
        // Vertical overlap of the window with this band, clipped to screen rows.
        let win_y1 = wy + wh;
        let y_start = core::cmp::max(wy, band_y0 as usize);
        let y_end = core::cmp::min(win_y1, band_y1 as usize);
        if y_start >= y_end { continue; }
        // Horizontal clip to the screen width.
        if wx >= sw { continue; }
        let vis_w = core::cmp::min(ww, sw - wx);
        let mut sy = y_start;
        while sy < y_end {
            let src_row_off = (sy - wy) * src_stride;
            let src_end = src_row_off + vis_w * 4;
            if src_end > win.px_len { break; }
            let src = win.px.add(src_row_off);
            let dst = back.add(sy * stride + wx * 4);
            // RGBA src → RGBX dst: identical byte order (alpha lands in the
            // ignored X slot). One row memcpy. (BGR conversion, if any, is done
            // once at present time by gfx::blit — the back-buffer is canonical
            // RGBX, matching the gate/gui surface format.)
            core::ptr::copy_nonoverlapping(src, dst, vis_w * 4);
            sy += 1;
        }
    }
}
```

- [ ] **Step 2: Register the module.** In `kernel/src/wasm/wt/mod.rs`, add after `pub mod wm;`:
```rust
pub mod compose;
```

- [ ] **Step 3: Compile check (mechanism only — no runtime yet).**
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem/kernel && source $HOME/.cargo/env && cargo build --release -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem --target x86_64-unknown-none 2>&1 | tail -15'
```
**Verify:** build succeeds (`Finished` line, no errors). The new module is dead code until Task 2 wires it — a `dead_code` warning on `composite_band`/`WinDesc` is expected and acceptable at this step.

---

## Task 2: The parallel dispatcher (BSP submits bands, joins, presents)

Build the BSP-side driver that snapshots the windows, encodes one band descriptor per job into a `'static` arena, submits them to the pool, joins, then presents the back-buffer with one `blit`. Includes the 1-CPU fallback (drain inline) so it is correct with no APs.

**Files:** Modify `kernel/src/wasm/wt/wm.rs`.

- [ ] **Step 1: Imports + the static job arena.** At the top of `kernel/src/wasm/wt/wm.rs`, after the existing `use` lines, add:
```rust
use core::sync::atomic::{AtomicU32, Ordering};
use crate::wasm::wt::compose::{composite_band, WinDesc};

/// Max composite bands per frame. One band per pool slot at most; capped well
/// under `pool::MAX_JOBS` (64) since a frame uses ~num-cores bands.
const MAX_BANDS: usize = 16;

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

/// Shared, snapshot window descriptors for the current frame. All band jobs of
/// one frame read the same window list (painter order). Sized for plenty of
/// windows; the BSP fills `[0, n)` then submits.
const MAX_WINS: usize = 64;
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
```

- [ ] **Step 2: The band job fn.** Add to `wm.rs` (a plain `fn` — no captures — matching `pool::JobFn = fn(&[u8]) -> u64`):
```rust
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
```

- [ ] **Step 3: The dispatcher.** Add to `wm.rs`. `windows` must already be sorted back-to-front (ascending z) by the caller; `back` is the screen-sized RGBX back-buffer.
```rust
/// Composite `windows` (back-to-front) into the screen-sized RGBX back-buffer
/// `back` (len = stride*screen_h), splitting the screen into horizontal bands
/// dispatched to the SMP pool. The BSP fills the static arenas, submits one job
/// per band, joins all, and returns. The caller then presents `back` with one
/// `gfx::blit`. App `frame()` scheduling is unaffected — only this raster runs
/// on the APs.
///
/// Correctness: bands have DISJOINT row ranges, every job writes only its own
/// rows ⇒ no two jobs touch the same back-buffer byte. The framebuffer itself
/// is NOT touched here (the BSP presents serially afterward).
fn composite_parallel(
    windows: &[Window],
    back: &mut [u8],
    stride: usize,
    screen_w: u32,
    screen_h: u32,
    bg: u32,
) {
    if screen_h == 0 || screen_w == 0 { return; }
    let n_wins = core::cmp::min(windows.len(), MAX_WINS);
    // 1) Snapshot the window descriptors into the shared WIN arena.
    // SAFETY: only the BSP writes WIN_ARENA, and only between frames (no job is
    // in flight — the previous frame was joined before we returned).
    for (i, w) in windows.iter().take(n_wins).enumerate() {
        unsafe {
            WIN_ARENA[i] = WinDesc {
                px: w.pixels.as_ptr(),
                px_len: w.pixels.len(),
                x: w.x,
                y: w.y,
                w: w.win_w,
                h: w.win_h,
            };
        }
    }
    let wins_ptr = core::ptr::addr_of!(WIN_ARENA) as usize; // *const WinDesc base
    let back_ptr = back.as_mut_ptr() as usize;

    // 2) Choose band count: one band per online core (incl. BSP), capped.
    // cpus_online() counts every registered core; ensure at least 1 band.
    let cores = core::cmp::max(crate::cpu::cpus_online(), 1) as usize;
    let n_bands = core::cmp::min(core::cmp::max(cores, 1), MAX_BANDS);
    let band_rows = (screen_h as usize + n_bands - 1) / n_bands; // ceil

    // 3) Fill the band arena + submit one job per band.
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

    // 4) 1-CPU (or pool-full) fallback: drain any queued jobs inline on the BSP
    // so we never deadlock waiting on cores that aren't there.
    if crate::cpu::cpus_online() <= 1 {
        while let Some(slot) = crate::smp::pool::take() {
            crate::smp::pool::run_slot(slot, crate::cpu::cpu_id());
        }
    }

    // 5) If submission stopped early (pool full), composite the leftover bands
    // inline on the BSP — correctness over parallelism.
    for b in n_submitted..n_bands {
        let y0 = (b * band_rows) as u32;
        if y0 >= screen_h { break; }
        let y1 = core::cmp::min(((b + 1) * band_rows) as u32, screen_h);
        // SAFETY: leftover bands are disjoint from submitted ones; BSP runs them
        // after submit, before join — but submitted jobs only touch THEIR rows.
        unsafe {
            composite_band(
                back.as_mut_ptr(),
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

    // 6) Join: block until every submitted band is DONE. poll_done frees slots.
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
```

- [ ] **Step 4: Wire SP3's `present_frame` to the parallel path.** Replace the body of SP3's serial `present_frame` (the function that today clears + blits each window) with the parallel composite + a single present. Locate SP3's `present_frame(windows: &[Window])` in `wm.rs`. If SP3 holds the back-buffer in the compositor state, reuse it; otherwise add a module-level back-buffer cache. Concretely, add this back-buffer helper + new body:
```rust
use alloc::vec::Vec;

/// Reused screen-sized RGBX back-buffer (grown once, kept across frames to avoid
/// per-frame allocation). Single-threaded on the BSP (only the dispatcher writes
/// it, via the band jobs, then the BSP reads it to present).
static mut BACK_BUFFER: Vec<u8> = Vec::new();

/// SP3 hook: compose all `windows` for one frame, then present to the screen.
/// SP4: the composite runs in parallel across the SMP pool; the present is one
/// serial blit on the BSP (which also recomposites the cursor).
fn present_frame(windows: &[Window]) {
    let g = crate::gfx::geom();
    let sw = g.width;
    let sh = g.height;
    if sw == 0 || sh == 0 { return; }
    let stride = (sw as usize) * 4; // canonical RGBX back-buffer pitch
    let needed = stride * sh as usize;
    // SAFETY: BSP-only access to BACK_BUFFER (no job touches this Vec handle;
    // jobs receive a raw pointer to its data, which stays valid while we hold
    // the Vec alive across the join in composite_parallel).
    let back: &mut Vec<u8> = unsafe { &mut *core::ptr::addr_of_mut!(BACK_BUFFER) };
    if back.len() < needed {
        back.resize(needed, 0);
    }

    // Painter order: sort window indices by ascending z (back-to-front).
    let mut order: Vec<usize> = (0..windows.len()).collect();
    order.sort_by_key(|&i| windows[i].z);
    let sorted: Vec<Window> = order.iter().map(|&i| Window {
        id: windows[i].id,
        x: windows[i].x,
        y: windows[i].y,
        win_w: windows[i].win_w,
        win_h: windows[i].win_h,
        pixels: windows[i].pixels.clone(), // snapshot this frame's surface
        z: windows[i].z,
        focused: windows[i].focused,
    }).collect();

    // Background: solid desktop colour (RGBX, little-endian). Dark slate.
    let bg: u32 = 0x00203040;
    composite_parallel(&sorted, &mut back[..needed], stride, sw, sh, bg);

    // Present: one serial blit of the whole back-buffer (recomposites cursor).
    crate::gfx::blit(&back[..needed], 0, 0, sw, sh);
}
```
> **NOTE for the implementer:** if SP3's `present_frame` already clones/snapshots `pixels` or already sorts by z, drop the redundant clone/sort here and pass SP3's existing sorted slice straight to `composite_parallel`. The `pixels.clone()` snapshot is the safe default (it removes any aliasing between the band jobs reading a window's surface and the BSP later mutating `WmState.pixels` in the next `frame()` call); if SP3 guarantees surfaces are immutable for the whole composite phase, the clone can be elided for performance. Keep the clone for the first correct landing.

- [ ] **Step 5: Build.**
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem/kernel && source $HOME/.cargo/env && cargo build --release -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem --target x86_64-unknown-none 2>&1 | tail -20'
```
**Verify:** `Finished` line, no errors. (If SP3 field names differ, fix the `Window` field reads flagged in the Assumed-interfaces section, then rebuild.)

---

## Task 3: Boot marker — prove the composite ran on ≥2 cores

Emit a serial `binfo!` line once compositing has run for a few frames, listing the distinct cores that executed band jobs. This is the SP4 analogue of `smptest`'s `cores=[...]` report — but driven from inside the compositor loop (no wasm tool needed; the compositor owns the CPU).

**Files:** Modify `kernel/src/wasm/wt/wm.rs` (the SP3 compositor loop / entry).

- [ ] **Step 1: Emit the marker after warm-up.** In SP3's compositor loop (the `loop { for each window: frame(); ... present_frame(...) }` body in the SP3 entry function), add a one-shot marker after a fixed number of frames so the core mask has accumulated. Add near the loop, using a local frame counter:
```rust
    let mut frame_no: u32 = 0;
    let mut marker_done = false;
    loop {
        // ... SP3: call each app frame(), copy committed surface into Window.pixels ...
        present_frame(&windows);
        frame_no += 1;
        // After 30 composited frames, report the distinct cores that ran band
        // jobs. One-shot; greppable by the boot-marker test. (cpu_id is
        // LAPIC-based per core, VBox-safe — see project memory.)
        if !marker_done && frame_no >= 30 {
            let mask = take_composite_core_mask();
            let mut cores: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
            for c in 0..32u32 {
                if mask & (1u32 << c) != 0 { cores.push(c); }
            }
            let n = cores.len();
            // Serial line the test greps: "wm  composite cores=N [..]"
            crate::binfo!("wm", "composite cores={} {:?}", n, cores);
            marker_done = true;
        }
        // ... SP3: pacing / input handling ...
    }
```
> The `binfo!("wm", ...)` module tag must be the literal `"wm"` (the boot logger takes a `$module:literal`). The test greps `wm  composite cores=`. `frame_no >= 30` gives the APs time to pick up several frames of jobs (the wake-IPI + worker-loop latency means the first frame or two may run partly inline on the BSP before APs warm up — 30 frames guarantees AP participation under `-smp 4`).

- [ ] **Step 2: Build + a quick boot smoke (serial only, no display).**
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && source $HOME/.cargo/env && make iso INIT_SCRIPT=user-bin/compositor-init.sh ISO=build/comptest.iso 2>&1 | tail -3'
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 40 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -boot d -cdrom build/comptest.iso -serial stdio -display none -no-reboot -m 512 -device qemu-xhci > build/comp-smp-serial.log 2>&1 || true; grep -E "wm  composite cores=" build/comp-smp-serial.log || echo NO_MARKER'
```
**Verify:** a line `wm  composite cores=N [..]` appears with `N >= 2` (e.g. `cores=4 [0, 1, 2, 3]`). If `NO_MARKER`, the compositor didn't reach 30 frames in 40s (raise the timeout) or the init script didn't launch `compositor` (check `build/comp-smp-serial.log` for `ruos boot OK`). If `N == 1`, the APs aren't picking up jobs — confirm `-smp 4` and that `cpus_online()` reports >1 earlier in the log (`smp  N/M APs online`).

---

## Task 4: Visual equivalence test — parallel screendump == serial screendump

Prove the parallel composite is pixel-identical to a serial composite. Strategy: capture a screendump in the default (parallel) build, and a second screendump from a serial reference build (a cargo feature that forces `n_bands = 1`, i.e. the whole screen as one band on the BSP). Identical windows ⇒ identical PNGs.

**Files:** Modify `kernel/src/wasm/wt/wm.rs` (add the `serial-composite` feature gate); modify `kernel/Cargo.toml` (declare the feature); create `tests/comp-smp-test.sh`; modify `Makefile` (`run-comp-smp-test`).

- [ ] **Step 1: A `serial-composite` feature to force one band.** In `kernel/Cargo.toml`, under `[features]`, add:
```toml
# Force the compositor to use a single band on the BSP (no SMP split). Used by
# the SP4 visual-equivalence test to capture a serial reference screendump.
serial-composite = []
```
In `composite_parallel` (Task 2 Step 3), gate the band count:
```rust
    // Choose band count: one band per online core (incl. BSP), capped.
    #[cfg(feature = "serial-composite")]
    let n_bands = 1usize;
    #[cfg(not(feature = "serial-composite"))]
    let n_bands = {
        let cores = core::cmp::max(crate::cpu::cpus_online(), 1) as usize;
        core::cmp::min(core::cmp::max(cores, 1), MAX_BANDS)
    };
```
Remove the old unconditional `let cores`/`let n_bands` two lines (replaced by the gated block above). The `band_rows` line below is unchanged.

- [ ] **Step 2: The test script.** Create `tests/comp-smp-test.sh`:
```bash
#!/usr/bin/env bash
# SP4: SMP-parallel compositing. Two assertions:
#  (A) boot marker shows >= 2 distinct cores ran composite band jobs (-smp 4);
#  (B) the parallel screendump is byte-identical to a serial-reference
#      screendump (same windows, n_bands forced to 1) -> compositing is correct.
set -u
cd "$(dirname "$0")/.."
QMP=/tmp/qmp.sock
for pid in $(pgrep -f 'qemu-system-x86_64'); do kill -9 "$pid" 2>/dev/null || true; done
sleep 1
rm -f build/comp-smp-serial.log build/shot.png build/shot-serial.png

# --- Build the PARALLEL comptest ISO (default features). ---
make iso INIT_SCRIPT=user-bin/compositor-init.sh ISO=build/comptest.iso > build/comp-iso.log 2>&1 \
  || { echo TEST_FAIL_ISO_PARALLEL; tail -20 build/comp-iso.log; exit 1; }

# Boot -smp 4 with QMP + serial; capture the boot marker AND a screendump.
rm -f "$QMP"
timeout 50 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -boot d \
  -cdrom build/comptest.iso -serial file:build/comp-smp-serial.log -display none \
  -no-reboot -m 512 -device qemu-xhci \
  -qmp unix:"$QMP",server,nowait &
QEMUPID=$!
# shot.py connects to /tmp/qmp.sock, waits ~16s, screendumps build/shot.png, quits.
python3 build/shot.py 16
wait "$QEMUPID" 2>/dev/null || true
# Rename the parallel shot before the serial run overwrites build/shot.png.
mv build/shot.png build/shot-parallel.png 2>/dev/null || mv build/shot.ppm build/shot-parallel.ppm 2>/dev/null || true

echo "=== boot marker ==="
marker=$(tr -d '\r' < build/comp-smp-serial.log | grep -oE 'wm  composite cores=[0-9]+ \[[0-9, ]*\]' | head -1)
echo "$marker"
ncores=$(echo "$marker" | grep -oE 'cores=[0-9]+' | grep -oE '[0-9]+')

# --- Build the SERIAL-REFERENCE comptest ISO (serial-composite feature). ---
make iso INIT_SCRIPT=user-bin/compositor-init.sh ISO=build/comptest-serial.iso \
  CARGO_FEATURES=serial-composite > build/comp-iso-serial.log 2>&1 \
  || { echo TEST_FAIL_ISO_SERIAL; tail -20 build/comp-iso-serial.log; exit 1; }
for pid in $(pgrep -f 'qemu-system-x86_64'); do kill -9 "$pid" 2>/dev/null || true; done
sleep 1
rm -f "$QMP"
timeout 50 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -boot d \
  -cdrom build/comptest-serial.iso -display none -no-reboot -m 512 -device qemu-xhci \
  -qmp unix:"$QMP",server,nowait &
QEMUPID=$!
python3 build/shot.py 16
wait "$QEMUPID" 2>/dev/null || true
mv build/shot.png build/shot-serial.png 2>/dev/null || mv build/shot.ppm build/shot-serial.ppm 2>/dev/null || true

# --- Assertions. ---
PAR=build/shot-parallel.png; SER=build/shot-serial.png
[ -f "$PAR" ] || PAR=build/shot-parallel.ppm
[ -f "$SER" ] || SER=build/shot-serial.ppm
identical=no
if [ -f "$PAR" ] && [ -f "$SER" ]; then
  if cmp -s "$PAR" "$SER"; then identical=yes; fi
fi
echo "distinct_cores=${ncores:-0} screendump_identical=$identical"
if [ "${ncores:-0}" -ge 2 ] && [ "$identical" = yes ]; then
  echo TEST_PASS_COMP_SMP
else
  echo TEST_FAIL_COMP_SMP
  echo "--- serial log tail ---"; tail -30 build/comp-smp-serial.log
  exit 1
fi
```
Make it executable:
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && chmod +x tests/comp-smp-test.sh'
```

- [ ] **Step 3: Makefile target.** In `Makefile`, add `run-comp-smp-test` to the `.PHONY` line (line 37) and add the target near the other `run-*-test` rules:
```makefile
# SP4: SMP-parallel compositing. Builds the comptest ISO twice (parallel default
# + serial-composite reference), boots -smp 4 with QMP, asserts >=2 cores ran the
# composite (boot marker) AND parallel screendump == serial screendump.
.PHONY: run-comp-smp-test
run-comp-smp-test:
	@bash tests/comp-smp-test.sh
```
> **NOTE:** if `CARGO_FEATURES` is not already plumbed into the `iso`/build rules (it IS used by `run-console-test`, so the variable exists), confirm `make iso CARGO_FEATURES=serial-composite` passes `--features serial-composite` to the kernel `cargo build`. If the `iso` target doesn't forward `CARGO_FEATURES` to the kernel build, add it to the kernel `cargo build` invocation the `iso` rule depends on (the `build` target). Grep `CARGO_FEATURES` in the Makefile to confirm; it is already consumed by `test-boot`/`run-console-test`.

- [ ] **Step 4: Run the full SP4 test.**
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-comp-smp-test 2>&1 | tail -25'
```
**Verify:** output ends with `TEST_PASS_COMP_SMP`, and the line above shows `distinct_cores=N screendump_identical=yes` with `N >= 2`. If `screendump_identical=no`, the parallel composite differs from serial — debug with `superpowers:systematic-debugging`: most likely a band-boundary off-by-one (a row composited by two bands or skipped) or a window straddling a band boundary not clipped to `[band_y0, band_y1)`. Inspect `build/shot-parallel.png` vs `build/shot-serial.png` (open both) and look for a horizontal seam at a band boundary (`screen_h / n_bands` row). If `distinct_cores < 2`, see Task 3 Step 2 troubleshooting.

- [ ] **Step 5: Inspect the parallel screendump visually.** Open `build/shot-parallel.png`: the windows must render exactly as SP3 produced them (no seams, no torn rows, no missing band). Send it for review if uncertain.

---

## Task 5: VirtualBox verification (CPU/MSR/STI-sensitive per project memory)

Project memory: *"Test VBox for CPU/MSR/STI-sensitive changes"* and *"core-id via LAPIC (NEVER gs:[0] — VBox quirk)"*. SP4 dispatches to APs (wake IPI, AP worker loop) and reads `cpu_id()` (LAPIC-based — already correct), so it MUST be verified on VirtualBox, not just QEMU.

**Files:** none (manual verification).

- [ ] **Step 1: Build the parallel comptest ISO** (already produced by Task 4 as `build/comptest.iso`). If absent:
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso INIT_SCRIPT=user-bin/compositor-init.sh ISO=build/comptest.iso 2>&1 | tail -3'
```

- [ ] **Step 2: Boot it in VirtualBox** with **≥ 4 vCPUs** and **EFI enabled** (per the SSD-installer memory: VBox needs EFI for UEFI boot; and SMP memory: test with 6 vCPU → APs online). Create/configure a VM pointing at `build/comptest.iso` as the optical drive. Confirm on the VBox serial log (or screen) that:
  - `smp  N/M APs online` shows APs came up;
  - `wm  composite cores=K [..]` shows `K >= 2` — i.e. APs ran composite jobs on real(ish) hardware via VBox, not just QEMU/KVM;
  - the two/N windows render correctly on screen (no seam, no garble).
- [ ] **Step 3: Record the VBox result** in the changelog (Task 6): vCPU count, `composite cores=K`, and that the windows rendered identically to QEMU. If VBox shows `K == 1` while QEMU shows `K >= 2`, the AP wake path differs under VBox — debug the wake IPI / `cpus_online` path (NOT the composite math, which is core-agnostic).

---

## Task 6: Changelog + finish

**Files:** Create `CHANGELOG/NN-26-06-05-compositor-sp4-smp-compositing.md` (next free `NN` — check `CHANGELOG/`, currently highest is `277`, so `278` unless taken).

- [ ] **Step 1: Write the changelog.** `CHANGELOG/278-26-06-05-compositor-sp4-smp-compositing.md`:
```markdown
# 278 — Compositor SP4: SMP-parallel compositing

**Data:** 2026-06-05

## Cosa
Il compositing per-frame di N finestre ora gira in parallelo sul compute-pool
SMP (gli AP). Lo schermo è diviso in bande orizzontali disgiunte; ogni banda è
un job pure-CPU (`fn(&[u8]) -> u64`) sottomesso a `crate::smp::pool` che compone
le finestre (z-order, painter's algorithm) nella sua porzione di un back-buffer
RGBX offscreen. Il BSP fa il join di tutte le bande e poi presenta il
back-buffer con un singolo `gfx::blit` (che ricompone anche il cursore). Lo
scheduling cooperativo degli `frame()` delle app resta sul BSP — solo il raster
va in parallelo. Bande disgiunte ⇒ nessuna race sul back-buffer; il framebuffer
reale è scritto solo dal BSP, una volta.

## Perché
Il compositing è lavoro pure-CPU spalmabile sui core (spec §3.3/§6). Sfrutta il
compute-pool Fase 2 esistente senza toccare l'executor del BSP.

## File toccati
- kernel/src/wasm/wt/compose.rs (nuovo: kernel pixel puro, banda)
- kernel/src/wasm/wt/wm.rs (dispatcher parallele + job + present_frame + marker)
- kernel/src/wasm/wt/mod.rs (pub mod compose)
- kernel/Cargo.toml (feature serial-composite per il test di equivalenza)
- tests/comp-smp-test.sh (nuovo: marker >=2 core + screendump identico)
- Makefile (run-comp-smp-test)
- CHANGELOG/278-26-06-05-compositor-sp4-smp-compositing.md

## Verifiche
- QEMU -smp 4: `wm  composite cores=4 [0,1,2,3]`; parallel screendump ==
  serial-reference screendump (byte-identical).
- VirtualBox >=4 vCPU + EFI: `composite cores=K (K>=2)`, finestre identiche.
```

- [ ] **Step 2: Final review.** Dispatch `superpowers:requesting-code-review` over `kernel/src/wasm/wt/compose.rs` + the `composite_parallel`/`composite_band_job`/`present_frame` additions in `wm.rs`, focusing on: (a) the `'static` arena soundness (BSP-only writes between joined frames), (b) band-boundary correctness (no row composited twice / skipped), (c) the `unsafe` raw-pointer contracts in `composite_band` (every write within `[band_y0, band_y1)`), (d) no framebuffer write off the BSP. Address findings via `superpowers:receiving-code-review`.

- [ ] **Step 3: Commit** (only when the user asks — do NOT auto-commit per CLAUDE.md). When asked:
```bash
cd /e/MinimalOS/BasicOperatingSystem && git add kernel/src/wasm/wt/compose.rs kernel/src/wasm/wt/wm.rs kernel/src/wasm/wt/mod.rs kernel/Cargo.toml tests/comp-smp-test.sh Makefile CHANGELOG/278-26-06-05-compositor-sp4-smp-compositing.md && git commit -m "feat(wm): SP4 — SMP-parallel per-band compositing"
```

---

## Provides (for later sub-projects)

SP4 exposes these interfaces for SP5 (launcher/lifecycle) and any later compositor work:

- `kernel/src/wasm/wt/compose.rs`:
  - `pub struct WinDesc { pub px: *const u8, pub px_len: usize, pub x: u32, pub y: u32, pub w: u32, pub h: u32 }` (`Send + Sync`).
  - `pub unsafe fn composite_band(back: *mut u8, stride: usize, screen_w: u32, band_y0: u32, band_y1: u32, bg: u32, wins: &[WinDesc])` — the pure, core-agnostic pixel kernel. SP5 can reuse it to composite an extra surface (e.g. a launcher panel / taskbar) by appending a `WinDesc` to the painter-ordered list.
- `kernel/src/wasm/wt/wm.rs`:
  - `fn present_frame(windows: &[Window])` — the SP3 hook, now SMP-parallel internally. SP5 calls it unchanged after spawning/placing a new window (the band split + present is transparent to the caller).
  - `pub fn take_composite_core_mask() -> u32` — read+clear the bitset of cores that ran composite jobs (boot-marker / telemetry; SP5 or a future `rtop`-style view can surface compositor core utilization).
  - Background colour `bg = 0x00203040` (RGBX) is the desktop fill SP5 draws the launcher over (or overrides).
- The compositing back-buffer is a kernel-owned screen-sized RGBX `Vec<u8>` (`stride = screen_w*4`), presented with one `crate::gfx::blit` per frame. SP5 should draw any always-on-top chrome (launcher bar) as a `WinDesc` in the painter list, NOT by blitting the framebuffer directly (which would be overwritten by the next present and would race the band jobs).
- Concurrency contract for later work: **only the BSP writes the real framebuffer** (via the single present `blit`); AP jobs write only the offscreen back-buffer, in disjoint bands. Any new parallel raster MUST preserve disjoint output partitioning + join-before-present.
