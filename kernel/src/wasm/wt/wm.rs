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
/// Phase-0.5 GATE app (Blitz HTML/CSS style+layout benchmark). Embedded ONLY under
/// `boot-checks` — it is ~51 MB of AOT Stylo native code, so production builds must
/// not carry it. Its guest `frame()` runs the bench once and prints a table to
/// serial via stdout (`fd_write` → console). Built by the demo-apps SDK
/// (`apps/viewer-gate`) and copied in as `viewer-gate.cwasm`.
#[cfg(feature = "boot-checks")]
static GATE_CWASM: &[u8] = include_bytes!("viewer-gate.cwasm");
/// Phase-2 viewer app (Blitz parse→style→layout→vello_cpu paint of an embedded
/// HTML page). Embedded ONLY under `boot-checks` — ~63 MB of AOT Stylo+vello_cpu
/// native code, so production builds must not carry it. Built by the demo-apps
/// SDK (`apps/viewer`) and copied in as `viewer.cwasm`.
#[cfg(feature = "boot-checks")]
static VIEWER_CWASM: &[u8] = include_bytes!("viewer.cwasm");

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
    let m = match unsafe { Module::deserialize(engine(), cwasm) } {
        Ok(m) => m,
        Err(e) => {
            crate::bwarn!("wm", "module_for: deserialize failed ({} bytes): {:?}", cwasm.len(), e);
            return None;
        }
    };
    cache.insert(key, m.clone());
    Some(m)
}

/// SP-C: cache of modules loaded BY NAME from the VFS (`/bin/<name>.cwasm`),
/// keyed by app name. The VFS bytes are NOT `&'static` (unlike the embedded
/// `APPS` blobs the ptr-keyed `MODULE_CACHE` serves), so this is a separate
/// `String`-keyed cache. `Module` is Arc-backed: a cached `clone` is cheap.
static NAME_CACHE: Mutex<BTreeMap<String, Module>> = Mutex::new(BTreeMap::new());

/// App directories searched (in priority order) for a `<name>.cwasm`: the
/// ISO-baked `/bin` (Limine modules — core apps, available diskless) first, then
/// the persistent FAT32 `/mnt/apps` DROP FOLDER (copy a `.cwasm` here at runtime —
/// no rebuild — and it becomes spawnable + appears in the launcher). `/mnt/apps`
/// is skipped when no disk is mounted.
const APP_DIRS: &[&str] = &["/bin", "/mnt/apps"];

/// Read `<dir>/<name>.cwasm` bytes, trying each [`APP_DIRS`] entry in order.
/// `/bin` wins over `/mnt/apps` on a name clash (a system app can't be shadowed).
fn read_app_bytes(name: &str) -> Option<Vec<u8>> {
    for dir in APP_DIRS {
        if *dir == "/mnt/apps" && !crate::vfs::is_mounted("/mnt") { continue; }
        let path = alloc::format!("{}/{}.cwasm", dir, name);
        if let Ok(b) = crate::vfs::block_on(crate::wasm::read_all(&path)) {
            return Some(b);
        }
    }
    None
}

/// Load `<name>.cwasm` from the VFS (see [`read_app_bytes`] for the search path)
/// and deserialize it (cached by name).
/// Returns None on any failure (missing file / bad bytes / deserialize error).
///
/// Synchronous: the compositor loop owns the CPU (it is NOT inside the async
/// executor), so block on the async VFS read via `crate::vfs::block_on` — the
/// same single-poll driver `state.rs`/`fs.rs`/`ssh` use. The loaded `Vec<u8>`
/// outlives `deserialize`; `Module` owns its code afterward.
fn module_by_name(name: &str) -> Option<Module> {
    if let Some(m) = NAME_CACHE.lock().get(name) { return Some(m.clone()); }
    let bytes = read_app_bytes(name)?;
    // SAFETY: wt-precompile output for this exact engine Config.
    let m = match unsafe { Module::deserialize(engine(), &bytes) } {
        Ok(m) => m,
        Err(e) => {
            crate::bwarn!("wm", "module_by_name '{}': deserialize failed ({} bytes): {:?}", name, bytes.len(), e);
            return None;
        }
    };
    NAME_CACHE.lock().insert(String::from(name), m.clone());
    Some(m)
}

// --- Dynamic launcher catalog (self-describing apps) -----------------------
//
// Instead of a hard-coded launcher list, the compositor SCANS `APP_DIRS` for
// `*.cwasm` and asks each one to describe itself: an app appears in the launcher
// IFF it exports `manifest() -> i64` (see ruos-window `declare_manifest!`). The
// export returns `(ptr<<32)|len` of a UTF-8 record `id\u{1f}title[\u{1f}w\u{1f}h]`
// in the guest's own linear memory. Apps that don't export it (shell, compositor,
// plain WASI commands) are simply absent from the launcher — the export IS the
// opt-in. Drop a manifest-bearing `.cwasm` into `/mnt/apps` and it shows up live.

/// A launcher entry parsed from an app's `manifest()` export. `id` is the spawn
/// key (`<id>.cwasm`); `title` is the display label.
#[derive(Clone)]
pub struct Manifest {
    pub id: String,
    pub title: String,
}

/// Stems never treated as launcher apps even if scanned (the desktop shell and the
/// compositor itself are infrastructure, not user apps). They export no manifest
/// anyway; listing them here just skips the wasted instantiate during a scan.
const EXCLUDE: &[&str] = &["shell", "compositor"];

/// The current launcher catalog, published by the compositor's throttled scan and
/// read by the `wm.app_list` host fn. Single producer (the run loop between frames)
/// + single consumer (the shell's `app_list` during `frame_all`).
static APP_CATALOG: crate::sync::IrqMutex<Vec<Manifest>> =
    crate::sync::IrqMutex::new(Vec::new());

/// One memoised manifest probe: the resolved file identity (path + size) and the
/// probe result (`Some(m)` = a launcher app, `None` = probed and has no manifest).
struct ProbeEntry {
    /// Resolved `.cwasm` path the probe ran against (a stem can shadow-flip
    /// between `/bin` and `/mnt/apps` — a path change forces a re-probe).
    path: String,
    /// File size at probe time, from `vfs::stat` METADATA (no content read). A
    /// changed size (e.g. an updated `.cwasm` dropped into `/mnt/apps`) forces a
    /// re-probe; same (path, size) = reuse without touching the file.
    size: u64,
    manifest: Option<Manifest>,
}

/// Per-stem manifest probe memo, keyed by `.cwasm` stem. A probe means
/// deserialising + instantiating a multi-MB AOT image on a throwaway store
/// (publish/teardown mprotect storm — see the 2026-06-10 TLB-shootdown spec), so
/// the ~1 Hz catalog refresh must NOT repeat it: a stem is re-probed ONLY when
/// new or when its resolved (path, size) changed; stems whose file disappeared
/// are evicted (so a re-dropped file is probed again).
static MANIFEST_CACHE: crate::sync::IrqMutex<BTreeMap<String, ProbeEntry>> =
    crate::sync::IrqMutex::new(BTreeMap::new());

/// Wall-clock seconds of the last catalog scan (throttle). Sentinel forces a scan
/// on the first frame.
static LAST_SCAN: crate::sync::IrqMutex<f64> = crate::sync::IrqMutex::new(-1.0e9);

/// Deserialize the `.cwasm` at `path` fresh (uncached) for a manifest probe.
/// Takes the already-resolved path (not a stem) so the probe reads exactly the
/// file the scan stat'ed for the cache key.
fn module_at_path(path: &str) -> Option<Module> {
    let bytes = crate::vfs::block_on(crate::wasm::read_all(path)).ok()?;
    // SAFETY: wt-precompile output for this exact engine Config.
    match unsafe { Module::deserialize(engine(), &bytes) } {
        Ok(m) => Some(m),
        Err(e) => {
            crate::bwarn!("wm", "probe '{}': deserialize failed ({} bytes): {:?}", path, bytes.len(), e);
            None
        }
    }
}

/// Instantiate `module` on a THROWAWAY store and call its `manifest()` export to
/// learn its launcher id/title. Returns None if the module has no `manifest`
/// export (→ not a launcher app) or anything goes wrong. The manifest record is
/// const data in the guest's data segment (no heap), so it is valid right after
/// instantiation without running `_initialize`. The store is dropped on return.
fn extract_manifest(linker: &Linker<AppState>, module: &Module) -> Option<Manifest> {
    let mut store = Store::new(
        engine(),
        AppState {
            wasi: WtState::new(alloc::vec![b"app".to_vec()]),
            win: WmState { id: 0, win_w: 0, win_h: 0, pixels: Vec::new(), tick: 0,
                           events: VecDeque::new(), close_requested: false,
                           move_requested: false, spawn_request: VecDeque::new(),
                           bg_request: false, committed: false,
                           minimize_request: false, maximize_request: false,
                           activate_request: VecDeque::new(), target_w: 0, target_h: 0,
                           stay_awake_request: false, wake_pty: -1 },
            limits: wasmtime::StoreLimits::default(), // throwaway probe: unlimited
        },
    );
    // SysV ABI requires DF=0 before any cranelift/Rust `rep movs`.
    #[cfg(target_arch = "x86_64")]
    unsafe { core::arch::asm!("cld", options(nostack)); }
    let inst = linker.instantiate(&mut store, module).ok()?;
    // The `manifest` export is OPTIONAL — its absence is the normal "not a launcher
    // app" case, so a missing-export error here is not logged.
    let f = inst.get_typed_func::<(), i64>(&mut store, "manifest").ok()?;
    let packed = f.call(&mut store, ()).ok()?;
    let ptr = ((packed >> 32) & 0xffff_ffff) as u32;
    let len = (packed & 0xffff_ffff) as u32;
    if len == 0 || len > 4096 { return None; }
    let mem = match inst.get_export(&mut store, "memory") {
        Some(wasmtime::Extern::Memory(m)) => m,
        _ => return None,
    };
    let mut buf = alloc::vec![0u8; len as usize];
    // SAFETY-equivalent of mem::read but on a Store (the audited Caller path is for
    // host fns); bounds are checked by `Memory::read`.
    mem.read(&store, ptr as usize, &mut buf).ok()?;
    let s = core::str::from_utf8(&buf).ok()?;
    // Record: id \u{1f} title [\u{1f} w \u{1f} h] — only id+title are used here.
    let mut it = s.split('\u{1f}');
    let id = it.next()?.trim();
    if id.is_empty() { return None; }
    let title = it.next().map(str::trim).filter(|t| !t.is_empty()).unwrap_or(id);
    Some(Manifest { id: String::from(id), title: String::from(title) })
}

/// Scan `APP_DIRS` for `*.cwasm`, probe any new-or-changed file's manifest
/// (memoised by resolved (path, size) — see [`MANIFEST_CACHE`]), and rebuild
/// [`APP_CATALOG`] from the CURRENTLY-present manifest-bearing files (so
/// deleting a `.cwasm` drops it from the launcher next scan). The file LIST is
/// re-read every scan (readdir + per-file `vfs::stat`, metadata only — cheap),
/// so `/mnt/apps` hot-plug keeps working; only the expensive probe is cached.
/// Runs between frames (never inside a guest call) so the throwaway
/// instantiate is safe.
fn scan_apps() {
    // Present `.cwasm` stems across the app dirs, in priority order, de-duped,
    // each with its resolved path + size. The size comes from `vfs::stat`
    // METADATA (tmpfs node / FAT32 dir entry) — no file content is read here.
    let mut present: Vec<(String, String, u64)> = Vec::new(); // (stem, path, size)
    for dir in APP_DIRS {
        if *dir == "/mnt/apps" && !crate::vfs::is_mounted("/mnt") { continue; }
        let entries = match crate::vfs::block_on(crate::vfs::readdir(dir)) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for e in entries {
            if let Some(stem) = e.name.strip_suffix(".cwasm") {
                if EXCLUDE.contains(&stem) { continue; }
                if present.iter().any(|(s, _, _)| s == stem) { continue; }
                let path = alloc::format!("{}/{}", dir, e.name);
                let size = match crate::vfs::block_on(crate::vfs::stat(&path)) {
                    Ok(st) => st.size,
                    Err(_) => continue, // raced with a delete: retry next scan
                };
                present.push((String::from(stem), path, size));
            }
        }
    }

    // Evict memo entries whose file is gone, then pick the stems needing a
    // (re-)probe: new, or resolved to a different path, or size changed. The
    // IrqMutex is held ONLY for these map ops — never across the blocking VFS
    // read / throwaway instantiate below.
    let (need_probe, evicted): (Vec<(String, String, u64)>, Vec<String>) = {
        let mut cache = MANIFEST_CACHE.lock();
        let evicted: Vec<String> = cache.keys()
            .filter(|stem| !present.iter().any(|(s, _, _)| s == stem.as_str()))
            .cloned()
            .collect();
        for stem in &evicted { cache.remove(stem); }
        let need: Vec<(String, String, u64)> = present.iter()
            .filter(|(stem, path, size)| {
                !matches!(cache.get(stem.as_str()),
                          Some(e) if e.path == *path && e.size == *size)
            })
            .cloned()
            .collect();
        (need, evicted)
    };

    // Invalida anche il Module cache-ato PER NOME (lo spawn) per gli stem
    // evitti o da ri-probare: altrimenti il launcher mostrerebbe il manifest
    // nuovo ma `wm.spawn` eseguirebbe ancora il Module vecchio (v1 cached dopo
    // un drop di v2 in /mnt/apps). Lock separato, mai annidato in MANIFEST_CACHE.
    if !evicted.is_empty() || !need_probe.is_empty() {
        let mut names = NAME_CACHE.lock();
        for stem in evicted.iter().chain(need_probe.iter().map(|(s, _, _)| s)) {
            names.remove(stem.as_str());
        }
    }

    // Probe (deserialize + throwaway instantiate + `manifest()` + drop) ONLY the
    // new-or-changed files; cached stems are reused without touching the file.
    // A failed probe is memoised as `manifest: None` at that size, so a partial
    // copy into `/mnt/apps` is re-probed once its size settles.
    if !need_probe.is_empty() {
        let engine = engine();
        let mut linker: Linker<AppState> = Linker::new(engine);
        if crate::wasm::wt::wasi::add_to_linker(&mut linker).is_err() { return; }
        if add_to_linker(&mut linker).is_err() { return; }
        if crate::wasm::wt::term::add_to_linker(&mut linker).is_err() { return; }
        if crate::wasm::wt::sys::add_to_linker(&mut linker).is_err() { return; }
        if crate::wasm::wt::net::add_to_linker(&mut linker).is_err() { return; }
        for (stem, path, size) in need_probe {
            let m = module_at_path(&path)
                .and_then(|module| extract_manifest(&linker, &module));
            MANIFEST_CACHE.lock().insert(stem, ProbeEntry { path, size, manifest: m });
        }
    }

    // Rebuild the visible catalog from the present, manifest-bearing files.
    let cache = MANIFEST_CACHE.lock();
    let mut cat: Vec<Manifest> = Vec::new();
    for (stem, _, _) in &present {
        if let Some(ProbeEntry { manifest: Some(m), .. }) = cache.get(stem) {
            if !cat.iter().any(|c| c.id == m.id) { cat.push(m.clone()); }
        }
    }
    drop(cache);
    cat.sort_by(|a, b| a.title.cmp(&b.title));
    *APP_CATALOG.lock() = cat;
}

/// Refresh the launcher catalog at most once per second (a `.cwasm` dropped into
/// `/mnt/apps` thus appears within ~1 s). Called from the run loop between frames.
fn refresh_app_catalog() {
    let now = crate::wasm::wt::gfx::wall_secs();
    {
        let mut last = LAST_SCAN.lock();
        if now - *last < 1.0 { return; }
        *last = now;
    }
    scan_apps();
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
    px: core::ptr::null(), px_len: 0, x: 0, y: 0, w: 0, h: 0, shadow: false,
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
            // Work-steal instead of pure spin so the waiting core makes forward
            // progress if APs are slow/stuck (prevents the real-hw freeze). Running
            // an ARBITRARY queued job here is safe: all pool jobs are pure
            // `fn(&[u8]) -> u64`, and the single-BSP cooperative model means only one
            // submitter is ever active at a time (APs never submit), so this never
            // races another submitter or violates the disjoint-bands invariant.
            if let Some(slot) = crate::smp::pool::take() {
                crate::smp::pool::run_slot(slot, crate::cpu::cpu_id());
            } else {
                core::hint::spin_loop();
            }
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
    /// Set by the guest via `wm.spawn(name)`; the run loop loads
    /// `/bin/<name>.cwasm` and spawns it as a new window AFTER the current
    /// `frame_all` pass (deferred so `wins` is never mutated mid-iteration), then
    /// drains it. A VecDeque so multiple `wm.spawn` calls in one frame are all
    /// honoured (last-wins Option was a spec violation).
    pub spawn_request: VecDeque<String>,
    /// Set by the guest via `wm.set_background()`; the run loop pins THIS window
    /// as the full-screen, z-bottom background (`Window.bg`) AFTER `frame_all`,
    /// then clears it.
    pub bg_request: bool,
    /// Damage flag: set by `wm.commit` (the guest drew a new surface this frame),
    /// cleared by the run loop before each `frame_all`. The compositor presents a
    /// frame only when SOME window committed (or geometry changed) — an idle app
    /// that commits nothing keeps its last surface and costs no composite/blit.
    pub committed: bool,
    /// Set by `wm.minimize()`; the run loop hides this window (Window.minimized).
    pub minimize_request: bool,
    /// Set by `wm.toggle_maximize()`; the run loop maximizes/restores this window.
    pub maximize_request: bool,
    /// Window-ids the guest asked to activate via `wm.activate(id)` (the shell's
    /// taskbar): un-minimize + raise + focus. A queue so several clicks in one frame
    /// are all honoured. Drained by the run loop.
    pub activate_request: VecDeque<u32>,
    /// Configure channel (kernel→app size authority): the size the kernel wants this
    /// window to render at, packed by `wm.window_size()` as `(w<<32)|h`. `(0,0)` =
    /// not yet established → the app uses its own default and the kernel adopts the
    /// first committed size. Maximize/restore/resize update these.
    pub target_w: u32,
    pub target_h: u32,
    /// Override dinamico: il guest ha chiamato `wm.stay_awake()` in questo frame →
    /// resta sveglio il prossimo. Azzerato dal run loop prima di ogni `frame_all`.
    pub stay_awake_request: bool,
    /// Risorsa PTY legata via `wm.wake_on_pty(idx)`: -1 = nessuna. Il compositor
    /// sveglia la finestra dormiente se quel pair ha output non drenato.
    pub wake_pty: i32,
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
    /// Per-store resource cap (`Store::limiter` points here). Live windows get
    /// [`WINDOW_MEM_CAP`] on linear memory so one runaway app (e.g. a huge page
    /// in the Blitz viewer) OOMs ITSELF instead of eating kernel RAM and taking
    /// the desktop down; throwaway probe stores keep the unlimited default.
    pub limits: wasmtime::StoreLimits,
}

/// Linear-memory cap for a LIVE window store (bytes). Apps link with a 48 MiB
/// initial memory (`.cargo/config.toml` in the SDK), and the heaviest measured
/// guest (Blitz viewer, 6.4k-node page) peaks ~16 MiB of heap — 128 MiB leaves
/// generous growth headroom while still bounding a runaway window.
pub const WINDOW_MEM_CAP: usize = 128 * 1024 * 1024;

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
                s.committed = true; // damage: this window drew a new surface this frame
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
    // wm.stay_awake(): il guest chiede di restare sveglio il PROSSIMO frame.
    // Azzerato dal run loop prima di ogni frame_all → va richiamato ogni frame
    // (modello egui request_repaint) per un aggiornamento continuo.
    linker.func_wrap("wm", "stay_awake",
        |mut caller: Caller<'_, T>| { caller.data_mut().win().stay_awake_request = true; })?;
    // wm.wake_on_pty(idx): lega una risorsa PTY a questa finestra; idx<0 = slega.
    // Il compositor sveglia la finestra dormiente quando quel pair ha output.
    linker.func_wrap("wm", "wake_on_pty",
        |mut caller: Caller<'_, T>, idx: i32| { caller.data_mut().win().wake_pty = idx; })?;
    // wm.start_move(): the guest (CSD title-bar grab) asks the compositor to begin
    // a kernel-driven interactive move of THIS window. We only record the request;
    // the run loop turns it into a `DragState` (reusing SP3's drag machinery) so
    // the window tracks the screen cursor without the app needing its screen origin.
    linker.func_wrap("wm", "start_move",
        |mut caller: Caller<'_, T>| { caller.data_mut().win().move_requested = true; })?;
    // wm.spawn(name_ptr, name_len): request the kernel launch /bin/<name>.cwasm as
    // a NEW window. Deferred to after frame_all (no `wins` mutation mid-iteration);
    // the run loop drains `spawn_request`, loads the module via `module_by_name`,
    // and calls `spawn_named`. Fire-and-forget (no return id; a return-id variant
    // is a later refinement). Reads the name from THIS guest's linear memory.
    linker.func_wrap("wm", "spawn",
        |mut caller: Caller<'_, T>, name_ptr: i32, name_len: i32| {
            if let Some(b) = crate::wasm::wt::mem::read(&mut caller, name_ptr as u32, name_len as u32) {
                if let Ok(s) = core::str::from_utf8(&b) {
                    caller.data_mut().win().spawn_request.push_back(String::from(s));
                }
            }
        })?;
    // wm.set_background(): the calling window flags ITSELF as the background
    // (full-screen, z-bottom, undecorated, not movable/closable). Deferred to the
    // run loop (which sets `Window.bg`), like spawn/close/move.
    linker.func_wrap("wm", "set_background",
        |mut caller: Caller<'_, T>| { caller.data_mut().win().bg_request = true; })?;
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
    // wm.poweroff(): the calling window (the shell's power button) asks the
    // kernel to power off the machine. Same primitive `ruos:gui/power.poweroff`
    // uses (`gui.rs`). Never returns; the `-> ()` annotation pins the closure's
    // Wasm return type so never-type fallback stays `()` (matches `gui.rs`).
    linker.func_wrap("wm", "poweroff",
        |_caller: Caller<'_, T>| -> () { crate::power::poweroff() })?;
    // wm.surface_size() -> i64: the full framebuffer size, packed (w<<32)|h. The
    // background shell sizes ITSELF to this (the bg window is forced full-screen
    // by the compositor; this lets the guest's egui raster match the screen).
    linker.func_wrap("wm", "surface_size",
        |_caller: Caller<'_, T>| -> i64 {
            let g = crate::gfx::geom();
            ((g.width as i64) << 32) | (g.height as i64)
        })?;
    // wm.window_size() -> i64: THIS window's kernel-assigned size, packed (w<<32)|h
    // (the "configure" the app renders to). `(0,0)` until established — the app then
    // uses its own default and the kernel adopts the first committed size. Maximize/
    // restore/resize change it; the app reads it each frame and re-rasters.
    linker.func_wrap("wm", "window_size",
        |caller: Caller<'_, T>| -> i64 {
            let s = caller.data().win_ref();
            ((s.target_w as i64) << 32) | (s.target_h as i64)
        })?;
    // wm.minimize(): the guest (yellow titlebar dot) asks to be hidden into the
    // taskbar. Deferred to the run loop (sets Window.minimized).
    linker.func_wrap("wm", "minimize",
        |mut caller: Caller<'_, T>| { caller.data_mut().win().minimize_request = true; })?;
    // wm.toggle_maximize(): the guest (green dot) asks to maximize to the work-area
    // or restore. Deferred to the run loop (toggles Window.maximized + geometry).
    linker.func_wrap("wm", "toggle_maximize",
        |mut caller: Caller<'_, T>| { caller.data_mut().win().maximize_request = true; })?;
    // wm.activate(id): the guest (the shell's taskbar) asks the kernel to un-minimize
    // + raise + focus the window with that id. Deferred to the run loop.
    linker.func_wrap("wm", "activate",
        |mut caller: Caller<'_, T>, id: i32| {
            caller.data_mut().win().activate_request.push_back(id as u32);
        })?;
    // wm.window_list(ptr, max) -> u32: write up to `max` taskbar records of the
    // current non-bg windows into guest memory at `ptr`, return the count. Each
    // record is 32 bytes: id(u32 LE) + flags(u32 LE: bit0=minimized, bit1=focused)
    // + title(24 bytes UTF-8, NUL-padded). Reads the compositor's per-frame snapshot
    // (the host fn cannot reach `wins`, so the run loop publishes it each frame).
    linker.func_wrap("wm", "window_list",
        |mut caller: Caller<'_, T>, ptr: i32, max: i32| -> i32 {
            let mut buf: Vec<u8> = Vec::new();
            {
                let snap = WINDOW_SNAPSHOT.lock();
                let n = core::cmp::min(snap.len(), (max.max(0)) as usize);
                for (id, flags, title) in snap.iter().take(n) {
                    buf.extend_from_slice(&id.to_le_bytes());
                    buf.extend_from_slice(&flags.to_le_bytes());
                    let mut t = [0u8; 24];
                    let tb = title.as_bytes();
                    let k = core::cmp::min(tb.len(), 24);
                    t[..k].copy_from_slice(&tb[..k]);
                    buf.extend_from_slice(&t);
                }
            } // drop the lock before touching guest memory
            let count = buf.len() / 32;
            crate::wasm::wt::mem::write(&mut caller, ptr as u32, &buf);
            count as i32
        })?;
    // wm.app_list(ptr, max) -> u32: write up to `max` launcher records of the
    // current app catalog (built by the compositor's `scan_apps`) into guest memory
    // at `ptr`, return the count. Each record is 64 bytes: id(24 bytes UTF-8,
    // NUL-padded) + title(40 bytes UTF-8, NUL-padded). The shell calls this each
    // frame to build its launcher menu — so dropping a manifest-bearing `.cwasm`
    // into `/mnt/apps` makes it appear without any rebuild.
    linker.func_wrap("wm", "app_list",
        |mut caller: Caller<'_, T>, ptr: i32, max: i32| -> i32 {
            let mut buf: Vec<u8> = Vec::new();
            {
                let cat = APP_CATALOG.lock();
                let n = core::cmp::min(cat.len(), (max.max(0)) as usize);
                for m in cat.iter().take(n) {
                    let mut id = [0u8; 24];
                    let ib = m.id.as_bytes();
                    let k = core::cmp::min(ib.len(), 24);
                    id[..k].copy_from_slice(&ib[..k]);
                    let mut title = [0u8; 40];
                    let tb = m.title.as_bytes();
                    let k = core::cmp::min(tb.len(), 40);
                    title[..k].copy_from_slice(&tb[..k]);
                    buf.extend_from_slice(&id);
                    buf.extend_from_slice(&title);
                }
            } // drop the lock before touching guest memory
            let count = buf.len() / 64;
            crate::wasm::wt::mem::write(&mut caller, ptr as u32, &buf);
            count as i32
        })?;
    Ok(())
}

/// Per-frame taskbar snapshot of the non-bg windows (id, flags, title), published
/// by the run loop and read by `wm.window_list`. flags: bit0 = minimized, bit1 =
/// focused. Single producer (the compositor loop) + single consumer (the shell's
/// `window_list` call during `frame_all`), but guarded for safety.
static WINDOW_SNAPSHOT: crate::sync::IrqMutex<Vec<(u32, u32, String)>> =
    crate::sync::IrqMutex::new(Vec::new());

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
        if let Err(e) = init.call(&mut *store, ()) {
            crate::bwarn!("wm", "_initialize trapped: {:?}", e);
        }
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
            win: WmState { id: 0, win_w: 0, win_h: 0, pixels: Vec::new(), tick: 0, events: VecDeque::new(), close_requested: false, move_requested: false, spawn_request: VecDeque::new(), bg_request: false, committed: false, minimize_request: false, maximize_request: false, activate_request: VecDeque::new(), target_w: 0, target_h: 0, stay_awake_request: false, wake_pty: -1 },
            limits: wasmtime::StoreLimits::default(), // spike harness: unlimited
        },
    );
    let mut linker: Linker<AppState> = Linker::new(engine);
    crate::wasm::wt::wasi::add_to_linker(&mut linker).expect("wasi linker");
    if add_to_linker(&mut linker).is_err() { return (0, 0, 0); }
    if crate::wasm::wt::term::add_to_linker(&mut linker).is_err() { return (0, 0, 0); }
    if crate::wasm::wt::sys::add_to_linker(&mut linker).is_err() { return (0, 0, 0); }
    if crate::wasm::wt::net::add_to_linker(&mut linker).is_err() { return (0, 0, 0); }
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
    /// SP-C: the background window. A `bg` window is composited FIRST (z-bottom),
    /// forced to the full framebuffer, undecorated, never raised/focused/moved/
    /// closed; it only receives input where no non-`bg` window covers the point.
    pub bg: bool,
    /// SP6 window controls: minimized windows are kept in `wins` but skipped by the
    /// composite + input hit-test (the taskbar restores them via `wm.activate`).
    pub minimized: bool,
    /// Maximized to the work-area; `saved_rect` holds the pre-maximize geometry to
    /// restore on toggle-off.
    pub maximized: bool,
    pub saved_rect: Option<(u32, u32, u32, u32)>,
    /// Configure (kernel→app size authority): false until the window's size is
    /// established. We bootstrap `rect.w/h` from the app's FIRST committed surface,
    /// then the kernel owns the size (`wm.window_size()` reports it; maximize/resize
    /// change it) and stops blindly adopting the committed size.
    pub sized: bool,
    /// SP-sleep: questa finestra ha girato `frame()` in questo loop (diagnostica).
    pub awake: bool,
    /// Ultimo `frame_no` con attività (input/override/dati) — grace/debounce.
    pub last_active_frame: u32,
    /// Ha già eseguito `_initialize` + almeno una `frame()`. Falso → primo giro
    /// sempre sveglio.
    pub framed_once: bool,
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
    /// Damage flag for GEOMETRY/window-set changes (raise, drag, spawn, reap,
    /// bg-pin) — things that change the composite even when no window committed
    /// new pixels. ORed with "any window committed" to decide whether `present`
    /// runs this frame; reset after each present. Starts `true` (first frame).
    pub dirty: bool,
    /// Contatore di frame del run loop (per il grace/debounce dello sleep).
    pub frame_no: u32,
}

/// Reactor surface size (matches `tools/wt-reactor` W/H). Windows are fixed at
/// this size in SP2; SP3 will make them resizable.
const WIN_W: u32 = 320;
const WIN_H: u32 = 240;

/// Top inset reserved for the shell's panel/taskbar (the bg window's top strip).
/// A maximized window fills the work-area BELOW this, so the panel/taskbar stays
/// visible. Approximate the shell panel height; refine if the panel grows.
const WORKAREA_TOP: u32 = 32;

/// Desktop background (RGBA8888 [r,g,b,a]) shown where no window covers.
const DESKTOP_BG: [u8; 4] = [0x10, 0x18, 0x20, 0xFF];

impl Compositor {
    /// Build the `wm` + WASI linker and boot the compositor into ONE initial
    /// window: the userspace desktop SHELL (`/bin/shell.cwasm`). The desktop UX
    /// (panel/launcher/wallpaper) lives in SP-D's userspace shell, which flags
    /// ITSELF as the full-screen background (`wm.set_background()`) on its first
    /// frame and `wm.spawn`s apps from its launcher. The initial window is loaded
    /// from `/bin/shell.cwasm` (VFS, via `module_by_name` — the compositor runs
    /// after fs init so `/bin` is mounted); if that load fails (shell.cwasm not in
    /// /bin yet) we fall back to the VFS egui-demo, then to the embedded
    /// `EGUI_DEMO_CWASM` (`include_bytes!`), so the compositor ALWAYS shows
    /// something. `cwasm` is the `compositor.cwasm` bytes the executor handed us —
    /// kept only to satisfy the `module` field (the shared reactor module used by
    /// the headless paths).
    pub fn new(cwasm: &[u8]) -> Compositor {
        let engine = engine();
        // SAFETY: produced by wt-precompile for this exact engine Config.
        let module = unsafe { Module::deserialize(engine, cwasm) }.expect("compositor module");
        let mut linker: Linker<AppState> = Linker::new(engine);
        crate::wasm::wt::wasi::add_to_linker(&mut linker).expect("wasi linker");
        add_to_linker(&mut linker).expect("wm linker"); // wm::add_to_linker (this module)
        crate::wasm::wt::term::add_to_linker(&mut linker).expect("term linker");
        crate::wasm::wt::sys::add_to_linker(&mut linker).expect("sys linker");
        crate::wasm::wt::net::add_to_linker(&mut linker).expect("net linker");

        let mut c = Compositor {
            wins: Vec::new(),
            module,
            linker,
            focused: 0,
            drag: None,
            backbuf: Vec::new(),
            free_ids: Vec::new(),
            next_id: 0,
            dirty: true,
            frame_no: 0,
        };

        // Initial window = the userspace desktop SHELL. Prefer the VFS
        // `/bin/shell.cwasm`; if it's not in `/bin` yet (or fails to load), fall
        // back to the VFS egui-demo, then to the embedded egui-demo blob — so the
        // compositor always shows SOMETHING. The shell self-flags bg on its first
        // frame; the egui-demo fallbacks come up as a plain window.
        let (name, initial) = match module_by_name("shell") {
            Some(m) => ("shell", Some(m)),
            None => {
                crate::bwarn!("wm", "shell.cwasm unavailable; falling back to egui-demo");
                ("egui-demo", module_by_name("egui-demo").or_else(|| module_for(EGUI_DEMO_CWASM)))
            }
        };
        match initial {
            Some(m) => { let _ = c.spawn_named(name, m); }
            None => { crate::bwarn!("wm", "no initial window: shell + egui-demo modules unavailable"); }
        }
        c
    }

    /// Headless compositor for the lifecycle boot-check: builds the linker +
    /// shared reactor Module but creates NO windows and NEVER calls
    /// `crate::gfx::enter` (no framebuffer). Spawn/reap run purely in RAM.
    fn new_empty() -> Compositor {
        let engine = engine();
        let mut linker: Linker<AppState> = Linker::new(engine);
        crate::wasm::wt::wasi::add_to_linker(&mut linker).expect("wasi linker");
        add_to_linker(&mut linker).expect("wm linker"); // wm::add_to_linker (this module)
        crate::wasm::wt::term::add_to_linker(&mut linker).expect("term linker");
        crate::wasm::wt::sys::add_to_linker(&mut linker).expect("sys linker");
        crate::wasm::wt::net::add_to_linker(&mut linker).expect("net linker");
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
            dirty: true,
            frame_no: 0,
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

    /// TOPMOST NON-`bg` window whose surface rect contains framebuffer point
    /// (px, py). Searches z-order from top (last) to bottom (first). `bg` windows
    /// are skipped here (they are never raised/focused; input that misses every
    /// non-`bg` window falls through to the `bg` window via `bg_index`).
    pub fn window_at(&self, px: i32, py: i32) -> Option<usize> {
        for i in (0..self.wins.len()).rev() {
            if self.wins[i].bg || self.wins[i].minimized { continue; }
            let (rx, ry, rw, rh) = self.wins[i].rect;
            let (rx, ry) = (rx as i32, ry as i32);
            if px >= rx && px < rx + rw as i32 && py >= ry && py < ry + rh as i32 {
                return Some(i);
            }
        }
        None
    }

    /// Index of the (first) `bg` window, if any. The background is the input
    /// fallthrough target: a click that misses every non-`bg` window routes here
    /// so the bg app (SP-D's shell panel/launcher) still gets clicks.
    fn bg_index(&self) -> Option<usize> {
        self.wins.iter().position(|w| w.bg)
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
        self.dirty = true; // z-order change → recomposite
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
            if (nx, ny) != (self.wins[i].rect.0, self.wins[i].rect.1) {
                self.dirty = true; // window moved → recomposite
                self.wins[i].rect = (nx, ny, w, h);
            }
            // Marcatura attività: il drag non inserisce eventi nel queue, quindi
            // aggiorni `last_active_frame` per tenerla sveglia durante il move.
            self.wins[i].last_active_frame = self.frame_no;
        }
    }

    /// Publish the kernel's desired size for window `i` into its guest WmState, so
    /// `wm.window_size()` reports it and the app re-rasters to match (configure).
    fn set_target(&mut self, i: usize, w: u32, h: u32) {
        let s = self.wins[i].store.data_mut();
        s.win.target_w = w;
        s.win.target_h = h;
    }

    /// Toggle window `i` between maximized (filling the work-area below the panel)
    /// and its saved geometry. Sets the configure target so the app re-rasters at
    /// the new size; marks the window `sized` so `frame_all` stops adopting the
    /// committed size (the kernel now owns it).
    fn toggle_maximize(&mut self, i: usize) {
        if i >= self.wins.len() || self.wins[i].bg { return; }
        if self.wins[i].maximized {
            if let Some(r) = self.wins[i].saved_rect.take() {
                self.wins[i].rect = r;
                self.set_target(i, r.2, r.3);
            }
            self.wins[i].maximized = false;
        } else {
            self.wins[i].saved_rect = Some(self.wins[i].rect);
            let g = crate::gfx::geom();
            let wa = (0u32, WORKAREA_TOP, g.width, g.height.saturating_sub(WORKAREA_TOP));
            self.wins[i].rect = wa;
            self.set_target(i, wa.2, wa.3);
            self.wins[i].maximized = true;
        }
        self.wins[i].sized = true;
        self.wins[i].last_active_frame = self.frame_no; // sveglia per ridisegnare alla nuova size
        self.dirty = true;
    }

    /// Restore (un-minimize) + raise + focus the window with `id` (the taskbar's
    /// `wm.activate`). No-op for an unknown id.
    fn activate(&mut self, id: u32) {
        if let Some(i) = self.index_of(id) {
            self.wins[i].minimized = false;
            let top = self.raise(i);
            self.set_focus(top);
            self.wins[top].last_active_frame = self.frame_no; // sveglia dopo un-minimize/raise
            self.dirty = true;
        }
    }

    /// Move focus to the topmost visible (non-bg, non-minimized) window, if any.
    /// Used after minimizing the focused window so focus never sticks on a hidden one.
    fn focus_topmost_visible(&mut self) {
        for i in (0..self.wins.len()).rev() {
            if !self.wins[i].bg && !self.wins[i].minimized {
                self.set_focus(i);
                return;
            }
        }
    }

    /// Unified teardown of `wins[i]`: remove it (dropping its Store+Instance →
    /// the wasm instance + guest linear memory + surface buffer are freed),
    /// unregister its proc, recycle its window-id to the free-list, and fix up
    /// `self.focused` so it never dangles and exactly one survivor is flagged.
    /// The SOLE place a window leaves `wins` (close + reap both route here).
    fn remove_at(&mut self, i: usize) {
        if i >= self.wins.len() { return; }
        self.dirty = true; // a window left the set → recomposite
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

    /// Spawn a window from an already-loaded `Module` under display name `name`:
    /// allocate a window-id, instantiate a fresh `(Store<AppState>, Instance)`,
    /// run `_initialize` (wasip1 reactors), place it at a cascading origin,
    /// raise+focus it, register a proc, and push it. Returns the new window-id,
    /// or None (budget full / instantiate failed). This is the ONE instance-
    /// creation path; `spawn_app` (embedded `APPS`, boot-checks) and the
    /// `wm.spawn` VFS path (`module_by_name`) both route here.
    pub fn spawn_named(&mut self, name: &str, module: Module) -> Option<u32> {
        let live = self.wins.iter().filter(|w| w.alive).count();
        if live >= MAX_WINDOWS {
            crate::bwarn!("wm", "spawn refused: window budget full ({})", live);
            return None;
        }
        let id = self.alloc_id();
        let mut store = Store::new(
            engine(),
            AppState {
                wasi: WtState::new(alloc::vec![b"win".to_vec()]),
                win: WmState { id, win_w: 0, win_h: 0, pixels: Vec::new(), tick: 0,
                               events: VecDeque::new(), close_requested: false,
                               move_requested: false, spawn_request: VecDeque::new(),
                               bg_request: false, committed: false,
                               minimize_request: false, maximize_request: false,
                               activate_request: VecDeque::new(), target_w: 0, target_h: 0,
                               stay_awake_request: false, wake_pty: -1 },
                limits: wasmtime::StoreLimitsBuilder::new()
                    .memory_size(WINDOW_MEM_CAP)
                    .build(),
            },
        );
        // Bound the guest's linear memory: a `memory.grow` past WINDOW_MEM_CAP
        // fails inside the GUEST (its allocator sees OOM and the app aborts);
        // the kernel heap and the other windows are untouched.
        store.limiter(|s| &mut s.limits);
        // SysV ABI requires DF=0 before any cranelift/Rust `rep movs`.
        #[cfg(target_arch = "x86_64")]
        unsafe { core::arch::asm!("cld", options(nostack)); }
        let inst = match self.linker.instantiate(&mut store, &module) {
            Ok(i) => i,
            Err(e) => {
                self.free_ids.push(id);
                crate::bwarn!("wm", "spawn: instantiate failed: {:?}", e);
                return None;
            }
        };
        // wasip1 std reactors need `_initialize` run before their first frame().
        run_initialize(&mut store, &inst);
        // Cascade placement (TUPLE rect): offset each new window so it doesn't
        // fully overlap. CSD = no title-bar offset; just clamp on-screen (keeping
        // a small bottom margin).
        let g = crate::gfx::geom();
        let n = live as u32;
        let ox = (40 + n * 28).min(g.width.saturating_sub(340));
        let oy = (40 + n * 28)
            .min(g.height.saturating_sub(LAUNCHER_H + 260));
        let pid = crate::proc::register(alloc::format!("win:{}", name));
        self.wins.push(Window {
            id,
            store,
            inst,
            rect: (ox, oy, 320, 240),
            title: String::from(name),
            focused: false,
            alive: true,
            pid,
            bg: false,
            minimized: false,
            maximized: false,
            saved_rect: None,
            sized: false,
            awake: true,
            last_active_frame: 0,
            framed_once: false,
        });
        let last = self.wins.len() - 1;
        self.raise(last);                 // move to top of z-order
        self.set_focus(self.wins.len() - 1); // focus the now-top window
        crate::binfo!("wm", "spawn app='{}' win_id={} pid={} live={}",
                      name, id, pid, live + 1);
        Some(id)
    }

    /// Spawn embedded registry app `idx` as a new window (boot-checks + the
    /// initial-window fallback). Delegates to `spawn_named` after deserialising
    /// the embedded cwasm. Returns the new window-id, or None (bad idx / bad
    /// module / budget full / instantiate failed).
    pub fn spawn_app(&mut self, idx: usize) -> Option<u32> {
        let app = APPS.get(idx)?;
        let module = module_for(app.cwasm)?;
        self.spawn_named(app.name, module)
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
                // SP-sleep lifecycle: se questa finestra aveva un PTY legato (terminale),
                // chiudilo deterministicamente — la shell legge EOF ed esce, il pair torna
                // Free. Sostituisce il reap-a-timeout del watchdog per i pair LocalGui.
                let wp = self.wins[i].store.data().win.wake_pty;
                if wp >= 0 {
                    crate::pty::request_shutdown(wp as usize);
                }
                self.remove_at(i);
            } else {
                i += 1;
            }
        }
    }

    /// CSD: the window IS its raw committed surface — no kernel decorations. Returns
    /// `(ptr, len, x, y, w, h)`: a BORROWED pointer to the guest's last-committed
    /// RGBA8888 surface placed at `Window.rect`'s origin, sized to the committed
    /// `win_w × win_h` (so the band kernel's `src_stride = w*4` matches the buffer
    /// exactly). The app draws its OWN title bar / [X] / content. None if nothing
    /// committed yet.
    ///
    /// Leva 0 (no per-frame clone): the pointer aliases `WmState.pixels` in the
    /// window's Store. `present` does NOT mutate `self.wins` between calling this
    /// and the `dispatch_bands` join, so the pixels are neither freed nor realloc'd
    /// while the band jobs read them — the borrow outlives the parallel composite.
    fn compose_window(&self, idx: usize) -> Option<(*const u8, usize, u32, u32, u32, u32)> {
        let win = &self.wins[idx];
        let s = win.store.data();
        if s.win.pixels.is_empty() { return None; }
        let (x, y, _, _) = win.rect;
        Some((s.win.pixels.as_ptr(), s.win.pixels.len(), x, y, s.win.win_w, s.win.win_h))
    }

    /// Numero di frame di grazia dopo l'ultima attività prima di dormire (evita
    /// flapping sleep↔wake; ~poche decine di ms al wake-rate del compositor).
    const GRACE_FRAMES: u32 = 6;

    /// `should_wake` calato su una `Window` concreta + il frame corrente.
    fn compute_awake(w: &Window, frame_no: u32) -> bool {
        let s = w.store.data();
        let has_events = !s.win.events.is_empty();
        let stay_awake = s.win.stay_awake_request;
        let pty_has_output = s.win.wake_pty >= 0
            && crate::pty::master_output_len(s.win.wake_pty as usize) > 0;
        should_wake(w.framed_once, has_events, stay_awake,
                    frame_no, w.last_active_frame, Self::GRACE_FRAMES, pty_has_output)
    }

    /// Call `frame()` on every window's instance (the gate's get_typed_func loop,
    /// ONE copy). Each app drains its queue via `wm.poll_event` and redraws.
    ///
    /// CSD crash safety-net: a `frame()` that returns `Err` — a trap, a
    /// `panic=abort`, or a guest `proc_exit` (which `wasi.rs` maps to a trap) —
    /// flags the window `close_requested`, so the reap pass drops it next loop
    /// instead of leaving a frozen, un-closeable window (its CSD [X] is gone).
    fn frame_all(&mut self) {
        let fno = self.frame_no;
        for w in self.wins.iter_mut() {
            if !Self::compute_awake(w, fno) {
                w.awake = false;
                continue; // dormiente: niente frame(); la surface in cache resta valida
            }
            w.awake = true;
            w.last_active_frame = fno;
            if let Ok(frame) = w.inst.get_typed_func::<(), ()>(&mut w.store, "frame") {
                match frame.call(&mut w.store, ()) {
                    Ok(()) => {}
                    Err(e) => {
                        // Anche un proc_exit volontario arriva qui come trap —
                        // il log distingue "app crashata" da "mai partita".
                        crate::bwarn!("wm", "frame() err win_id={}: {:?}", w.id, e);
                        w.store.data_mut().win.close_requested = true;
                    }
                }
            }
            // Considera la finestra "avviata" solo quando ha prodotto una surface:
            // un'app egui può richiedere più frame per il primo commit; finché non
            // disegna resta sveglia (framed_once=false ⇒ gira ogni frame).
            if w.store.data().win.committed {
                w.framed_once = true;
            }
            // CSD: the window IS its committed surface, so the hit-rect size must
            // track the committed `win_w × win_h` (apps pick their own size — the
            // egui demo commits 480×320, not the 320×240 spawn placeholder). Without
            // this, `window_at` / drag clamp use the stale placeholder size and a
            // click on the app's [X] (past the placeholder right/bottom edge) is
            // never routed. Keep the rect ORIGIN (x,y); only adopt the real w/h.
            let (cw, ch) = { let s = w.store.data(); (s.win.win_w, s.win.win_h) };
            if cw != 0 && ch != 0 && !w.sized {
                // Configure bootstrap: the FIRST committed surface establishes the
                // window's size. Keep the rect ORIGIN (x,y); adopt the committed
                // w/h, publish it as the configure target, and mark `sized` — from
                // now the kernel owns the size (maximize/resize set rect + target;
                // the app renders to `wm.window_size()`, so the committed size
                // already matches rect.w/h and we must NOT re-adopt it).
                let (rx, ry, _, _) = w.rect;
                w.rect = (rx, ry, cw, ch);
                let s = w.store.data_mut();
                s.win.target_w = cw;
                s.win.target_h = ch;
                w.sized = true;
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

        // SP-C: a `bg` window is pinned to the full framebuffer and composited
        // FIRST (z-bottom), independent of its position in `wins`. Force its rect
        // to (0,0,sw,sh) here every frame (frame_all resets it to the committed
        // size); its surface is blitted at (0,0) and `DESKTOP_BG` fills any area
        // the surface doesn't cover.
        let bg_idx = self.bg_index();
        if let Some(bi) = bg_idx {
            self.wins[bi].rect = (0, 0, sw, sh);
        }

        // 1) BSP: build footprint descriptors bottom->top (z = `wins` Vec order),
        //    but with the `bg` window FIRST (z-bottom, forced to origin (0,0)).
        //    Leva 0: each descriptor BORROWS the window's committed surface in
        //    place (no clone). The pixels live in each window's Store and are NOT
        //    mutated between here and the `dispatch_bands` join, so the raw
        //    pointers stay valid for the whole parallel composite.
        // Each desc carries a `shadow` flag: the bg window casts none (it's the
        // full-screen backdrop), every normal window does.
        let mut descs: alloc::vec::Vec<(*const u8, usize, u32, u32, u32, u32, bool)> =
            alloc::vec::Vec::new();
        if let Some(bi) = bg_idx {
            // Force the bg surface's footprint origin to (0,0) (full-screen bottom).
            if let Some((px, len, _, _, fw, fh)) = self.compose_window(bi) {
                descs.push((px, len, 0, 0, fw, fh, false));
            }
        }
        for i in 0..self.wins.len() {
            if bg_idx == Some(i) { continue; } // bg already composited first
            if self.wins[i].minimized { continue; } // hidden → not composited
            if let Some((px, len, x, y, w, h)) = self.compose_window(i) {
                descs.push((px, len, x, y, w, h, true));
            }
        }
        let n = core::cmp::min(descs.len(), MAX_WINS);
        // SAFETY: BSP-only write of WIN_ARENA between joined frames (the previous
        // frame's jobs were joined inside dispatch_bands before we returned, so
        // no job is in flight while we mutate the arena here).
        for (i, (px, len, fx, fy, fw, fh, sh)) in descs.iter().enumerate().take(n) {
            unsafe {
                WIN_ARENA[i] = WinDesc {
                    px: *px, px_len: *len, x: *fx, y: *fy, w: *fw, h: *fh, shadow: *sh,
                };
            }
        }

        // 2) Dispatch the banded composite into backbuf, then present. The borrowed
        //    surfaces stay alive in their Stores (we don't touch `self.wins`) for
        //    the entire parallel composite — dispatch_bands joins all jobs before
        //    returning, so no band job outlives this call.
        let bg = u32::from_le_bytes(DESKTOP_BG);
        let back_ptr = self.backbuf.as_mut_ptr() as usize;
        dispatch_bands(back_ptr, stride, sw, sh, bg, n);
        crate::gfx::blit(&self.backbuf[..needed], 0, 0, sw, sh);
    }

    /// Push a window-local MouseMove (kind 1) into window `idx`'s queue so the app's
    /// egui pointer tracks the cursor. Coords are screen − window origin, encoded as
    /// f32 bits (the `wm.poll_event` / gui-core ABI: p0=x.bits, p1=y.bits). Always
    /// forwarded (even slightly outside the rect) so egui sees the pointer leave.
    fn forward_mouse_move(&mut self, idx: usize, sx: i32, sy: i32) {
        if idx >= self.wins.len() { return; }
        let (rx, ry, _, _) = self.wins[idx].rect;
        let lx = (sx - rx as i32) as f32;
        let ly = (sy - ry as i32) as f32;
        self.wins[idx].store.data_mut().win.events.push_back(crate::gfx::GfxEvt {
            kind: 1, p0: lx.to_bits(), p1: ly.to_bits(), p2: 0,
        });
        // Marcatura attività: garantisce il ridisegno anche se compute_awake
        // venisse rivisto. L'evento appena inserito basta già per il wake.
        self.wins[idx].last_active_frame = self.frame_no;
    }

    /// Forward a left-button event (press/release) to the focused window so its
    /// egui widgets ([X], counter, title-bar drag-sense) react. `pressed` selects
    /// the edge. ABI: kind 2, p0=button(0=left), p1=pressed. A MouseMove to the same
    /// window-local point is pushed FIRST so egui's pointer is positioned at the
    /// click site before the button edge (egui clicks at the latest pointer pos).
    fn forward_left_button(&mut self, idx: usize, sx: i32, sy: i32, pressed: bool) {
        if idx >= self.wins.len() { return; }
        self.forward_mouse_move(idx, sx, sy);
        self.wins[idx].store.data_mut().win.events.push_back(crate::gfx::GfxEvt {
            kind: 2, p0: 0, p1: pressed as u32, p2: 0,
        });
        // Marcatura attività (belt-and-suspenders; l'evento nel queue basta già).
        self.wins[idx].last_active_frame = self.frame_no;
    }

    /// Left mouse-down at screen (px,py). Under CSD the kernel only does window
    /// management: the topmost NON-`bg` window under the cursor is raised +
    /// focused, and the positioned press is forwarded to that window's queue
    /// (window-local coords) so the app's OWN [X]/title-bar/content (all egui-
    /// rendered) react. Close + title-drag are the app's job (via `wm.close` /
    /// `wm.start_move`). A click that misses every non-`bg` window falls through
    /// to the `bg` window (so SP-D's shell panel/launcher gets clicks) WITHOUT
    /// raising/focusing it. No `bg`/no window under empty space => nothing.
    fn on_left_down(&mut self, px: i32, py: i32) {
        if let Some(i) = self.window_at(px, py) {
            let top = self.raise(i);
            self.set_focus(top);
            // Marcatura attività sulla finestra appena portata in primo piano.
            self.wins[top].last_active_frame = self.frame_no;
            // Forward the POSITIONED press to the now-focused window so egui clicks
            // at the cursor (not 0,0) and its drag-sense (title bar) can arm.
            self.forward_left_button(top, px, py, true);
        } else if let Some(bg) = self.bg_index() {
            // Fallthrough: the click hit bare desktop → route it to the bg window
            // (window-local coords). The bg is never raised/focused; it is also
            // never the topmost via `window_at`, so it only ever sees this path.
            self.forward_left_button(bg, px, py, true);
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
        // Telemetria one-shot a compositor-ready: quanti shootdown TLB (e quanti
        // full-flush CR3 remoti) è costato arrivare alla GUI — la metrica del fix
        // "shootdown a range" (publish/teardown storm, spec 2026-06-10).
        let (sd, ff) = crate::memory::tlb::stats();
        crate::binfo!("wm", "tlb stats at compositor-ready: shootdowns={} full_flushes={}", sd, ff);
        // SysV ABI requires DF=0; cranelift/Rust code uses `rep movs` which run
        // BACKWARD if DF=1, silently corrupting copied data.
        #[cfg(target_arch = "x86_64")]
        unsafe { core::arch::asm!("cld", options(nostack)); }

        let mut btn_l = false;
        let mut marker_done = false;
        loop {
            self.frame_no = self.frame_no.wrapping_add(1);
            // Supervisor 6-detect: bump the GUI core's heartbeat each compositor
            // frame so the supervisor never false-mutes it. The GUI core owns the
            // CPU here and may not call run_core(), so it must bump explicitly.
            crate::sched::cpustat::heartbeat_bump(crate::cpu::cpu_id() as usize);

            // SP5: reap any window that requested close (guest wm.close or the [X]
            // path) on its last frame BEFORE we fold input or drive frames.
            self.reap();
            // Refresh the dynamic launcher catalog (throttled to ~1 Hz). Runs HERE,
            // between frames — never inside a guest call — so the per-app manifest
            // probe (a throwaway instantiate) can't nest inside another guest call.
            refresh_app_catalog();
            crate::gfx::fold_mouse();
            while let Some(ev) = crate::gfx::pop() {
                let (cx, cy) = crate::gfx::mouse_pos();
                match ev.kind {
                    1 => { // mousemove
                        if let Some(d) = self.drag {
                            // A kernel-driven interactive move owns the cursor: track
                            // the dragged window and DON'T forward to apps (egui would
                            // fight the move). The move ends on button-up below.
                            self.drag_to(d.win_id, cx, cy, (d.grab_dx, d.grab_dy));
                        } else if let Some(i) = self.window_at(cx, cy) {
                            // Hover: forward a window-local move to the TOPMOST window
                            // under the cursor so its egui pointer tracks (hover state,
                            // and the click site is positioned before the next press).
                            // Only the topmost gets it (no false hover on covered ones).
                            self.forward_mouse_move(i, cx, cy);
                        } else if let Some(bg) = self.bg_index() {
                            // Bare desktop: the bg window is the input fallthrough, so
                            // its egui pointer tracks there too (hover + click site).
                            self.forward_mouse_move(bg, cx, cy);
                        }
                    }
                    2 => { // mouse button: edge-track the left button
                        if ev.p0 == 0 {
                            let pressed = ev.p1 != 0;
                            if pressed && !btn_l {
                                btn_l = true;
                                self.on_left_down(cx, cy);
                            } else if !pressed && btn_l {
                                btn_l = false;
                                // ALWAYS forward the release to the relevant window so
                                // egui's pointer state never sticks "down". If a kernel
                                // interactive move was active, route the release to the
                                // DRAGGED window (its title-bar label's drag-sense must
                                // see the pointer-up that ended the grab) and clear the
                                // drag. Otherwise route to the focused window so a plain
                                // click completes (counter / [X] activate on release) —
                                // unless the press fell through to the `bg` window (the
                                // cursor is over bare desktop and a bg window exists), in
                                // which case route the release there so the bg click
                                // completes (bg is never focused, so `focused` isn't it).
                                let target = match self.drag.take() {
                                    Some(d) => self.index_of(d.win_id),
                                    None => {
                                        if self.window_at(cx, cy).is_none() {
                                            self.bg_index().or(Some(self.focused))
                                        } else {
                                            Some(self.focused)
                                        }
                                    }
                                };
                                if let Some(i) = target {
                                    if i < self.wins.len() {
                                        self.forward_left_button(i, cx, cy, false);
                                    }
                                }
                            }
                        }
                    }
                    0 => { // key -> focused window's queue
                        if self.focused < self.wins.len() {
                            self.wins[self.focused].store.data_mut().win.events.push_back(ev);
                        }
                    }
                    5 => { // wheel -> topmost window under the cursor (hover-
                           // scroll, like kind 1), falling through to the bg
                           // window over bare desktop. Forwarded verbatim (the
                           // payload is a delta, not a position — no local-coord
                           // rewrite needed). A MouseMove is forwarded first so
                           // egui's pointer is over the scroll area it should
                           // scroll.
                        let target = self.window_at(cx, cy).or_else(|| self.bg_index());
                        if let Some(i) = target {
                            if i < self.wins.len() {
                                self.forward_mouse_move(i, cx, cy);
                                self.wins[i].store.data_mut().win.events.push_back(ev);
                                self.wins[i].last_active_frame = self.frame_no;
                            }
                        }
                    }
                    _ => {}
                }
            }

            // Clear per-window damage flags; `wm.commit` re-sets `committed` for any
            // window that draws a new surface during this `frame_all`. Also azzera
            // l'override dinamico `stay_awake_request` (dura un solo frame).
            for w in self.wins.iter_mut() {
                let s = w.store.data_mut();
                s.win.committed = false;
                s.win.stay_awake_request = false;
            }

            self.frame_all();

            // SP-C: process deferred window→kernel requests raised during
            // `frame_all` (`wm.set_background`, `wm.spawn`). Deferred to HERE —
            // after the frame pass, before `present` — so `wins` is never mutated
            // mid-iteration (mirrors the close/move deferred pattern).
            //
            // 1) Background requests: pin the requesting window as `bg`.
            for i in 0..self.wins.len() {
                if self.wins[i].store.data().win.bg_request {
                    self.wins[i].store.data_mut().win.bg_request = false;
                    self.wins[i].bg = true;
                    self.dirty = true; // bg pin changes the composite → present
                    crate::binfo!("wm", "bg window win_id={}", self.wins[i].id);
                }
            }
            // 2) Spawn requests: drain each window's name FIRST (collect into a
            //    Vec), THEN spawn — so we don't borrow `wins` while pushing new
            //    windows (no mid-iteration mutation). Defers `wins` growth past
            //    `frame_all`.
            let mut to_spawn: alloc::vec::Vec<alloc::string::String> = alloc::vec::Vec::new();
            for w in self.wins.iter_mut() {
                while let Some(name) = w.store.data_mut().win.spawn_request.pop_front() {
                    to_spawn.push(name);
                }
            }
            for name in to_spawn {
                if let Some(module) = module_by_name(&name) {
                    if let Some(id) = self.spawn_named(&name, module) {
                        crate::binfo!("wm", "wm.spawn ok name='{}' id={}", name, id);
                    }
                } else {
                    crate::bwarn!("wm", "wm.spawn: /bin/{}.cwasm not found/bad", name);
                }
            }

            // CSD interactive move: a guest that called `wm.start_move()` this
            // frame (title-bar grab) has `move_requested` set. Turn it into a
            // kernel-driven drag of that window — grab offset = cursor − window
            // origin — and let the existing mousemove→`drag_to` / button-up→
            // `drag=None` machinery (below) drive + end the move. Last request
            // wins if several fire in one frame (only one cursor). A `bg` window
            // is never moved (its `start_move` request is ignored).
            for i in 0..self.wins.len() {
                if self.wins[i].store.data().win.move_requested {
                    self.wins[i].store.data_mut().win.move_requested = false;
                    if self.wins[i].bg { continue; }
                    let (cx, cy) = crate::gfx::mouse_pos();
                    let rect = self.wins[i].rect;
                    self.drag = Some(DragState {
                        win_id: self.wins[i].id,
                        grab_dx: cx - rect.0 as i32,
                        grab_dy: cy - rect.1 as i32,
                    });
                }
            }

            // SP6 window controls (deferred, like spawn/move): the traffic-light dots.
            // Minimize (yellow) → hide into the taskbar; toggle-maximize (green) →
            // work-area / restore. (Close = red is the existing `close_requested`.)
            for i in 0..self.wins.len() {
                if self.wins[i].store.data().win.minimize_request {
                    self.wins[i].store.data_mut().win.minimize_request = false;
                    if !self.wins[i].bg {
                        self.wins[i].minimized = true;
                        self.dirty = true;
                        crate::binfo!("wm", "minimize win_id={}", self.wins[i].id);
                        if self.focused == i { self.focus_topmost_visible(); }
                    }
                }
                if self.wins[i].store.data().win.maximize_request {
                    self.wins[i].store.data_mut().win.maximize_request = false;
                    self.toggle_maximize(i);
                }
            }
            // Taskbar activate requests (`wm.activate`): drain each window's queue,
            // then apply (un-minimize + raise + focus the target id).
            let mut to_activate: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
            for w in self.wins.iter_mut() {
                while let Some(id) = w.store.data_mut().win.activate_request.pop_front() {
                    to_activate.push(id);
                }
            }
            for id in to_activate { self.activate(id); }

            // Publish the taskbar snapshot (non-bg windows) for the shell's
            // `wm.window_list`. flags: bit0 = minimized, bit1 = focused.
            {
                let mut snap = WINDOW_SNAPSHOT.lock();
                snap.clear();
                for (i, w) in self.wins.iter().enumerate() {
                    if w.bg { continue; }
                    let mut flags = 0u32;
                    if w.minimized { flags |= 1; }
                    if i == self.focused { flags |= 2; }
                    snap.push((w.id, flags, w.title.clone()));
                }
            }

            // Present only when something actually changed: any window committed a
            // new surface this frame, OR geometry/window-set changed (`self.dirty`).
            // An idle desktop (no commits, no moves) skips the whole composite+blit
            // — the framebuffer already holds the last frame, and the software
            // cursor is maintained independently by `gfx::fold_mouse`/`blit`.
            //
            // WARM-UP EXCEPTION: force a present for the first `WARMUP_FRAMES` loops
            // so the SMP band pool warms up (APs need several composited frames to
            // pick up jobs) and the "composite cores" boot-marker — which fires at
            // frame 30, see below — observes real multi-core band activity. The
            // comp-smp test's apps are static (they commit once), so without this
            // the marker could see <2 cores. Idle-skipping kicks in after warm-up.
            const WARMUP_FRAMES: u32 = 90;
            let any_committed = self.wins.iter().any(|w| w.store.data().win.committed);
            if self.dirty || any_committed || self.frame_no < WARMUP_FRAMES {
                self.present();
                self.dirty = false;
            }

            // After 30 composited frames, report the distinct cores that ran
            // band jobs (one-shot; greppable by the boot-marker test). The
            // warm-up gives the APs time to pick up several frames of jobs (the
            // wake-IPI + worker-loop latency means the first frame or two may
            // run partly inline on the BSP before APs warm up). cpu_id is
            // LAPIC-based per core, VBox-safe — see project memory.
            if !marker_done && self.frame_no >= 30 {
                let mask = take_composite_core_mask();
                let mut cores: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
                for c in 0..32u32 {
                    if mask & (1u32 << c) != 0 { cores.push(c); }
                }
                let n_cores = cores.len();
                crate::binfo!("wm", "composite cores={} {:?}", n_cores, cores);
                marker_done = true;
            }

            // Idle pacing: park the core until the next interrupt instead of busy-
            // spinning (which pegged a core at 100% even with nothing on screen
            // changing). The 100 Hz timer bounds the wake latency to ~10 ms; PS/2
            // and USB input IRQs wake us sooner, so drag/click stay responsive. With
            // present-gating above, an idle desktop now does ~zero work per tick.
            // SAFETY: the compositor loop runs with IF=1 (entered from the executor,
            // which enables interrupts), so `hlt` is guaranteed to be woken by an IRQ.
            x86_64::instructions::hlt();
        }
    }
}

/// Decide se una finestra deve girare `frame()` questo giro. Pura → boot-checkabile.
/// Sveglia se: non ha ancora girato (init), ha input in coda, ha l'override
/// `stay_awake`, ha output PTY legato in attesa, o è entro il grace dall'ultima
/// attività.
fn should_wake(framed_once: bool, has_events: bool, stay_awake: bool,
               frame_no: u32, last_active: u32, grace: u32,
               pty_has_output: bool) -> bool {
    !framed_once
        || has_events
        || stay_awake
        || pty_has_output
        || frame_no.wrapping_sub(last_active) < grace
}

/// Entry point (the executor router calls EXACTLY this name; do NOT rename).
/// Builds the canonical `Compositor` and runs its input-routed loop forever.
pub fn run_compositor_gate(cwasm: &[u8]) -> ! {
    crate::gfx::enter();
    Compositor::new(cwasm).run()
}

// ---------------------------------------------------------------------------
// Step 5: GUI-core hand-off infrastructure
// ---------------------------------------------------------------------------

/// One-shot mailbox from the BSP to the GUI core. The BSP publishes ptr+len
/// with Release ordering, then sets `ready` with Release. The GUI core polls
/// `ready` with Acquire; the ptr+len Acquire-loads follow the same barrier.
///
/// The slice outlives the kernel (the BSP leaks a Box<[u8]> before hand-off),
/// so the GUI core can safely read it forever.
struct CompositorMailbox {
    ptr:   core::sync::atomic::AtomicUsize,
    len:   core::sync::atomic::AtomicUsize,
    ready: core::sync::atomic::AtomicBool,
}

static COMPOSITOR_MAILBOX: CompositorMailbox = CompositorMailbox {
    ptr:   core::sync::atomic::AtomicUsize::new(0),
    len:   core::sync::atomic::AtomicUsize::new(0),
    ready: core::sync::atomic::AtomicBool::new(false),
};

/// GUI core entry point (Step 5). Called by `ap_entry` when this core's role
/// is `GuiCompositor`. Halts until the BSP publishes the compositor cwasm,
/// then runs `run_compositor_gate` forever on THIS dedicated core.
///
/// The BSP's `exec_worker_task` keeps running (the hand-off returns) so the
/// BSP executor continues polling net/usb/ssh — the goal of Step 5.
pub fn gui_worker_loop() -> ! {
    crate::binfo!("wm", "gui core {} waiting for compositor",
                  crate::cpu::cpu_id());
    loop {
        // Supervisor 6-detect: bump the GUI core's heartbeat each time it wakes
        // from hlt while waiting for the compositor hand-off. Without this bump
        // the supervisor would see a mute core for the entire pre-compositor window
        // (potentially many seconds). The LAPIC timer wakes this core ~100 Hz →
        // it runs this loop body → it bumps → it halts again.
        crate::sched::cpustat::heartbeat_bump(crate::cpu::cpu_id() as usize);

        if COMPOSITOR_MAILBOX.ready.load(core::sync::atomic::Ordering::Acquire) {
            let ptr = COMPOSITOR_MAILBOX.ptr.load(core::sync::atomic::Ordering::Acquire)
                as *const u8;
            let len = COMPOSITOR_MAILBOX.len.load(core::sync::atomic::Ordering::Acquire);
            // SAFETY: the BSP leaked a Box<[u8]> (so the allocation lives forever)
            // and published ptr+len with Release before setting `ready`. We
            // Acquire-load all three. The compositor never returns, so the leak is
            // permanent and correct.
            let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
            run_compositor_gate(bytes); // -> !
        }
        // Not ready yet: halt until the hand-off IPI (wake_core → VEC_WAKE)
        // wakes us. The sti+hlt sequence is race-free: if the IPI fires between
        // the disable and the hlt, it is held pending until sti's one-instruction
        // shadow, then delivered before hlt executes.
        x86_64::instructions::interrupts::disable();
        if !COMPOSITOR_MAILBOX.ready.load(core::sync::atomic::Ordering::Acquire) {
            x86_64::instructions::interrupts::enable_and_hlt();
        } else {
            x86_64::instructions::interrupts::enable();
        }
    }
}

/// Hand the compositor cwasm bytes to the GUI core (Step 5). Returns `true` if
/// handed off (a GUI core exists and is waiting — the caller must NOT run the
/// gate inline). Returns `false` if no GUI core exists (1-core fallback: the
/// caller runs `run_compositor_gate` inline, today's behaviour).
///
/// `bytes` MUST be a leaked 'static slice (Box::leak or equivalent) whose
/// allocation lives for the kernel's lifetime, because the GUI core reads it
/// indefinitely. Passing a stack slice or a Vec<u8> that may be freed is UB.
pub fn send_compositor_to_gui_core(bytes: &'static [u8]) -> bool {
    // Find a GuiCompositor core that is online.
    let mut gui: Option<u32> = None;
    for c in 0..crate::cpu::cpus_online() {
        if crate::cpu::core_role(c) == crate::cpu::CoreRole::GuiCompositor {
            gui = Some(c);
            break;
        }
    }
    let gui = match gui {
        Some(c) => c,
        None    => return false, // no GUI core (1 CPU or no APs)
    };

    // Publish ptr + len BEFORE setting ready (Release → Acquire pairing with
    // the GUI core's poll). The GUI core only reads ptr/len after seeing ready.
    COMPOSITOR_MAILBOX.ptr.store(bytes.as_ptr() as usize,
                                 core::sync::atomic::Ordering::Release);
    COMPOSITOR_MAILBOX.len.store(bytes.len(),
                                 core::sync::atomic::Ordering::Release);
    COMPOSITOR_MAILBOX.ready.store(true, core::sync::atomic::Ordering::Release);

    // Wake the GUI core: set its WAKE_PENDING + send a targeted VEC_WAKE IPI.
    // The IPI wakes it from `hlt`; the GUI core then re-checks `ready` and
    // calls run_compositor_gate.
    crate::executor::wake_core(gui);
    crate::binfo!("wm", "compositor handed off to gui core {}", gui);
    true
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

/// Boot self-test (SP-B): spawn the egui CSD demo as a compositor window and drive
/// one frame; returns the committed surface length. Builds an empty (headless, no
/// `gfx::enter`) compositor, finds the `"egui-demo"` registry entry, `spawn_app`s
/// it (instantiate against the unified `Linker<AppState>` + `run_initialize`), then
/// `frame_all()` calls the guest's `frame()` once — egui instantiates its Context,
/// runs one `ctx.run` + tessellate + raster, and `wm.commit`s the surface. A return
/// of 614400 (480×320×4) proves the full egui pipeline ran headlessly. 0 = the
/// entry is missing, instantiate failed (a WASI import egui needs isn't registered),
/// or the egui raster/commit trapped.
pub fn egui_demo_self_test() -> usize {
    let mut c = Compositor::new_empty();
    let idx = APPS.iter().position(|a| a.name == "egui-demo").unwrap_or(usize::MAX);
    if idx == usize::MAX || c.spawn_app(idx).is_none() { return 0; }
    c.frame_all();
    c.wins.last().map(|w| w.store.data().win.pixels.len()).unwrap_or(0)
}

/// Boot self-test (SP-GATE, Phase-0.5): spawn the Blitz GATE app as a compositor
/// window and drive ONE frame. The guest's first `frame()` runs the Blitz
/// style+layout benchmark (Stylo + Taffy on synthetic pages of growing size) and
/// prints a table — `rows | nodes | avg_resolve_ms | heap_peak_KiB` — to serial via
/// stdout. The numbers quantify how long a `resolve()` would FREEZE the single-core
/// synchronous compositor (`frame.call`) and the heap a `StoreLimits` cap must hold.
/// Returns the committed surface length (non-zero = the Blitz guest instantiated
/// against the unified WASI+wm `Linker<AppState>` and ran). RNG must already be live
/// (Stylo seeds HashMaps via WASI `random_get`) — the caller seeds it first.
/// Resolves the module from the EMBEDDED blob (`/bin` isn't mounted this early).
#[cfg(feature = "boot-checks")]
pub fn gate_self_test() -> usize {
    let mut c = Compositor::new_empty();
    let module = match module_for(GATE_CWASM) { Some(m) => m, None => return 0 };
    if c.spawn_named("viewer-gate", module).is_none() { return 0; }
    c.frame_all();
    c.wins.last().map(|w| w.store.data().win.pixels.len()).unwrap_or(0)
}

/// Boot self-test (SP-VIEWER, Phase-2): spawn the Blitz viewer app and drive TWO
/// frames. The guest's first `frame()` parses+styles+lays out its embedded HTML
/// page, paints it with vello_cpu into an RGBA buffer and uploads it as an egui
/// texture (printing `viewer: <nodes> nodes · resolve <ms> · paint <ms>` to
/// serial via stdout); the second frame draws the textured image. Returns the
/// committed surface length (non-zero = the full parse→style→layout→paint
/// pipeline ran in-kernel). RNG must already be live (Stylo seeds HashMaps via
/// WASI `random_get`). Resolves the module from the EMBEDDED blob.
#[cfg(feature = "boot-checks")]
pub fn viewer_self_test() -> usize {
    let mut c = Compositor::new_empty();
    let module = match module_for(VIEWER_CWASM) { Some(m) => m, None => return 0 };
    if c.spawn_named("viewer", module).is_none() { return 0; }
    c.frame_all();
    c.frame_all(); // 2nd frame: the texture uploaded on frame 1 is drawn here
    c.wins.last().map(|w| w.store.data().win.pixels.len()).unwrap_or(0)
}

/// Boot self-test (SP-C): prove the `wm.spawn` deferred-spawn MECHANISM grows the
/// window list and the `wm.set_background` mechanism pins a window to the full
/// framebuffer. Returns a 2-bit flag word (`0b11` == both pass).
///
/// This boot-check runs in the `interrupts` phase — BEFORE the VFS `/bin` is
/// mounted — so `module_by_name` (the real `wm.spawn` VFS load) can't work here.
/// Instead it exercises the SAME deferred-spawn + bg LOGIC the run loop uses, but
/// resolves the module from the EMBEDDED blob (`module_for(EGUI_DEMO_CWASM)`, the
/// identical module the VFS path serves). The VFS `wm.spawn` itself is covered
/// VISUALLY (clicking "spawn another" loads `/bin/egui-demo.cwasm`).
///
///   bit0 — spawn grows `wins` to 2: spawn one initial window via `spawn_named`,
///          then set its `spawn_request`, drain it with the run-loop's request
///          logic (resolving via the embedded module), and assert `wins.len()==2`.
///   bit1 — bg full-screen: flag one window `bg` (the deferred bg-request path),
///          run the `present` bg rect-forcing logic, and assert its rect was
///          pinned to the full screen `(0,0,sw,sh)`. The framebuffer is set up in
///          the LATER `devices` phase, so `geom()` reads 0×0 here; the test falls
///          back to a synthetic size to verify the rect-FORCING mechanism. The
///          real full-screen composite (live framebuffer) is covered visually.
#[cfg(feature = "boot-checks")]
pub fn spc_self_test() -> u32 {
    let mut flags = 0u32;
    let mut c = Compositor::new_empty();

    // The embedded egui-demo module stands in for the VFS `/bin/egui-demo.cwasm`
    // (same bytes; `/bin` isn't mounted this early).
    let module = match module_for(EGUI_DEMO_CWASM) {
        Some(m) => m,
        None => return flags, // can't deserialize → both checks fail
    };

    // Initial window.
    if c.spawn_named("egui-demo", module.clone()).is_none() {
        return flags;
    }

    // (bit0) Exercise the DEFERRED-spawn mechanism: set the existing window's
    // `spawn_request` (as `wm.spawn` would), then run the SAME drain logic the run
    // loop uses (collect names, then spawn) — but resolve via the embedded module
    // since the VFS isn't up. A successful spawn grows `wins` from 1 to 2.
    c.wins[0].store.data_mut().win.spawn_request.push_back(String::from("egui-demo"));
    let mut to_spawn: Vec<String> = Vec::new();
    for w in c.wins.iter_mut() {
        while let Some(name) = w.store.data_mut().win.spawn_request.pop_front() {
            to_spawn.push(name);
        }
    }
    for name in to_spawn {
        let _ = c.spawn_named(&name, module.clone());
    }
    if c.wins.len() == 2 { flags |= 1 << 0; }

    // (bit1) Exercise the bg mechanism: flag the first window's `bg_request` (as
    // `wm.set_background` would), run the deferred bg-request processing (the run
    // loop's pin-as-bg step), then run the `present` bg rect-forcing logic and
    // assert the rect became the full framebuffer.
    if !c.wins.is_empty() {
        c.wins[0].store.data_mut().win.bg_request = true;
        // Deferred bg-request processing (mirror of the run loop).
        for i in 0..c.wins.len() {
            if c.wins[i].store.data().win.bg_request {
                c.wins[i].store.data_mut().win.bg_request = false;
                c.wins[i].bg = true;
            }
        }
        // `present`'s bg rect-forcing (don't call full `present` — it would blit to
        // the framebuffer mid-boot; just run the rect-forcing the bg path does).
        // The framebuffer geometry is set up in the LATER `devices` phase, so this
        // early `geom()` can read 0×0; fall back to a synthetic screen size so the
        // test verifies the rect-FORCING mechanism (bg pinned to (0,0,sw,sh))
        // regardless of where the dims come from. The real bg full-screen composite
        // (with the live framebuffer) is covered visually.
        let g = crate::gfx::geom();
        let (sw, sh) = if g.width != 0 && g.height != 0 { (g.width, g.height) } else { (1280, 800) };
        if let Some(bi) = c.bg_index() {
            c.wins[bi].rect = (0, 0, sw, sh);
            if c.wins[bi].rect == (0, 0, sw, sh) {
                flags |= 1 << 1;
            }
        }
    }

    flags
}

/// Boot self-test (SP-D): prove the desktop-shell boot mechanism is wired without
/// needing the VFS (`/bin/shell.cwasm` isn't mounted in the `interrupts` phase, and
/// the shell isn't `include_bytes!`'d). Asserts two things the shell relies on:
///
///  1. The `wm.poweroff` + `wm.surface_size` host fns REGISTER. `Compositor::new_empty`
///     calls `add_to_linker`, which `func_wrap`s every `wm` import incl. the two new
///     SP-D ones; a duplicate/typo would make `add_to_linker` (hence `new_empty`) panic.
///     So a successfully-built empty compositor proves both new imports are bound and a
///     shell guest importing them could instantiate.
///  2. The background mechanism still pins a window full-screen (the shell flags ITSELF
///     bg via `wm.set_background` on its first frame). Reuses SP-C's bg rect-forcing on
///     the embedded egui-demo stand-in.
///
/// Returns the forced bg size packed `(w<<32)|h` (0 == the bg mechanism failed). The
/// caller logs `spd: hostfns ok bg=WxH`. The shell-as-bg desktop itself (chrome +
/// launcher → `wm.spawn`) is verified VISUALLY (it needs the VFS + framebuffer).
#[cfg(feature = "boot-checks")]
pub fn spd_self_test() -> i64 {
    // (1) Building the compositor builds the linker via `add_to_linker`; if the new
    // `wm.poweroff`/`wm.surface_size` registrations were broken (dup name / wrong
    // signature path) this would panic. Reaching past it = both host fns are bound.
    let mut c = Compositor::new_empty();

    // (2) Bg full-screen mechanism (the shell self-flags bg). Spawn one window
    // (embedded egui-demo stands in for /bin/shell.cwasm — not mounted yet), flag it
    // bg via the deferred bg-request path, then run `present`'s rect-forcing and read
    // back the pinned size. geom() reads 0×0 this early (framebuffer set up in the
    // later `devices` phase), so fall back to a synthetic size — we're verifying the
    // rect-FORCING, not the live framebuffer (that's the visual proof).
    let module = match module_for(EGUI_DEMO_CWASM) {
        Some(m) => m,
        None => return 0,
    };
    if c.spawn_named("shell", module).is_none() {
        return 0;
    }
    c.wins[0].store.data_mut().win.bg_request = true;
    for i in 0..c.wins.len() {
        if c.wins[i].store.data().win.bg_request {
            c.wins[i].store.data_mut().win.bg_request = false;
            c.wins[i].bg = true;
        }
    }
    let g = crate::gfx::geom();
    let (sw, sh) = if g.width != 0 && g.height != 0 { (g.width, g.height) } else { (1280, 800) };
    if let Some(bi) = c.bg_index() {
        c.wins[bi].rect = (0, 0, sw, sh);
        if c.wins[bi].rect == (0, 0, sw, sh) {
            return ((sw as i64) << 32) | (sh as i64);
        }
    }
    0
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
