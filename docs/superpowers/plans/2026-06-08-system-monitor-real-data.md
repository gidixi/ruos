# System Monitor real data (SP-F) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the System Monitor window's simulated data with live kernel telemetry (per-core CPU, processes, memory, uptime), keeping `gui-core` pure.

**Architecture:** A new Wasmtime host module `sys` (kernel) exposes the same blob layouts rtop already uses. `gui-core` defines a pure `SysInfoSource` capability trait + POD snapshots and renders from them; `ruos-window` implements the trait over the `sys` host fns; `system-app` wires the real source in. The window diffs two samples over wall time for per-process CPU% and per-core utilisation. PC preview drops System (no simulated source shipped).

**Tech Stack:** Rust `no_std` kernel (wasmtime no_std AOT), Rust std `wasm32-wasip1` (gui-core/ruos-window/system-app), egui + egui_extras, tiny-skia. Build: `cargo` (host, for gui-core tests) + WSL `make iso` (kernel + `.cwasm`).

**Repos / branches:**
- Kernel: `W:\Work\GitHub\ruos`, branch **`feat/system-monitor-real-data`** (already checked out).
- Submodule: `W:\Work\GitHub\ruos\ruos-desktop`, create branch **`feat/system-monitor-real-data`** (Task 4).

**Spec:** `docs/superpowers/specs/2026-06-08-system-monitor-real-data-design.md`

**Blob layouts (shared contract, mirrored from `kernel/src/wasm/host/sysinfo.rs`):**
- `cpustat`: `u32 ncores`, `u64 tsc_per_ms`, then `ncores × (u64 busy, u64 idle)`.
- `proc_stat`: `u32 count`, then per row `u32 pid, u64 start_tick, u64 cpu_tsc, u64 mem_bytes, u16 name_len, u16 pad, name[name_len]`.
- `meminfo`: `u64 heap_total, u64 heap_used, u64 frames_total, u64 frames_used` (32 bytes; `heap_used` is 0 — talc has no used-bytes API).
- `uptime`: `i64` return = centiseconds since boot.

---

## Repo A — kernel (ruos)

### Task 1: Kernel `sys` host module

**Files:**
- Create: `kernel/src/wasm/wt/sys.rs`
- Modify: `kernel/src/wasm/wt/mod.rs` (declare the module)

- [ ] **Step 1: Create `kernel/src/wasm/wt/sys.rs`**

```rust
//! `sys` host module for compositor (Wasmtime) windows: live CPU/memory/process
//! telemetry for the System Monitor app. Mirrors the wasmi `host/sysinfo.rs` blob
//! layouts so the guest parser is the same shape, but for the `wt` `Linker<T>`
//! (generic over the store type; reads global kernel state, writes guest memory
//! via the single audited `wt::mem` path). Errno-style returns: 0 ok, 8 ERANGE,
//! 21 EFAULT (guest write out of bounds).

use wasmtime::{Caller, Linker};
use alloc::vec::Vec;

/// Register `sys.{cpustat,proc_stat,meminfo,uptime}` on a window linker. Generic
/// over `T` (these fns never touch the store data — only global kernel state +
/// guest memory), so the same call works for `Linker<AppState>` and any other.
pub fn add_to_linker<T>(linker: &mut Linker<T>) -> wasmtime::Result<()> {
    // sys.cpustat(buf_ptr, buf_len) -> i32: u32 ncores, u64 tsc_per_ms, then
    // ncores × (u64 busy, u64 idle). 8 = ERANGE if the buffer is too small.
    linker.func_wrap("sys", "cpustat",
        |mut caller: Caller<'_, T>, buf_ptr: i32, buf_len: i32| -> i32 {
            let ncores = 1 + crate::cpu::cpus_online() as usize;
            let mut blob: Vec<u8> = Vec::new();
            blob.extend_from_slice(&(ncores as u32).to_le_bytes());
            blob.extend_from_slice(&crate::boot::clock::tsc_per_ms().to_le_bytes());
            for cpu in 0..ncores {
                let (busy, idle) = crate::sched::cpustat::read(cpu);
                blob.extend_from_slice(&busy.to_le_bytes());
                blob.extend_from_slice(&idle.to_le_bytes());
            }
            if (buf_len.max(0) as usize) < blob.len() { return 8; }
            if !crate::wasm::wt::mem::write(&mut caller, buf_ptr as u32, &blob) { return 21; }
            0
        })?;
    // sys.proc_stat(buf_ptr, buf_len, used_ptr) -> i32: u32 count then per-row
    // pid/start_tick/cpu_tsc/mem_bytes/name. Writes the FULL required length to
    // used_ptr; truncates the bytes to buf_len so the guest can size up + retry.
    linker.func_wrap("sys", "proc_stat",
        |mut caller: Caller<'_, T>, buf_ptr: i32, buf_len: i32, used_ptr: i32| -> i32 {
            let procs = crate::proc::list();
            let mut blob: Vec<u8> = Vec::new();
            blob.extend_from_slice(&(procs.len() as u32).to_le_bytes());
            for p in &procs {
                blob.extend_from_slice(&p.pid.to_le_bytes());
                blob.extend_from_slice(&p.start_tick.to_le_bytes());
                blob.extend_from_slice(&p.cpu_tsc.to_le_bytes());
                blob.extend_from_slice(&p.mem_bytes.to_le_bytes());
                let name = p.name.as_bytes();
                blob.extend_from_slice(&(name.len() as u16).to_le_bytes());
                blob.extend_from_slice(&[0u8, 0u8]); // pad
                blob.extend_from_slice(name);
            }
            let n = blob.len().min(buf_len.max(0) as usize);
            if !crate::wasm::wt::mem::write(&mut caller, buf_ptr as u32, &blob[..n]) { return 21; }
            if !crate::wasm::wt::mem::write_u32(&mut caller, used_ptr as u32, blob.len() as u32) { return 21; }
            0
        })?;
    // sys.meminfo(buf_ptr) -> i32: 4 × u64 (heap_total, heap_used, frames_total,
    // frames_used). heap_used = 0 (talc has no stable used-bytes API in our cfg).
    linker.func_wrap("sys", "meminfo",
        |mut caller: Caller<'_, T>, buf_ptr: i32| -> i32 {
            let heap_total = crate::memory::HEAP_SIZE as u64;
            let heap_used: u64 = 0;
            let frames = crate::memory::frame_counts();
            let mut out = [0u8; 32];
            out[0..8].copy_from_slice(&heap_total.to_le_bytes());
            out[8..16].copy_from_slice(&heap_used.to_le_bytes());
            out[16..24].copy_from_slice(&frames.total.to_le_bytes());
            out[24..32].copy_from_slice(&frames.used.to_le_bytes());
            if !crate::wasm::wt::mem::write(&mut caller, buf_ptr as u32, &out) { return 21; }
            0
        })?;
    // sys.uptime() -> i64: centiseconds since boot (100 Hz tick → already cs).
    linker.func_wrap("sys", "uptime",
        |_caller: Caller<'_, T>| -> i64 { crate::timer::ticks() as i64 })?;
    Ok(())
}
```

- [ ] **Step 2: Declare the module in `kernel/src/wasm/wt/mod.rs`**

Modify the module list (currently ends at `pub mod compose;`, lines 5-14) to add `sys`:

```rust
pub mod platform;
pub mod state;
pub mod mem;
pub mod wasi;
pub mod gfx;
pub mod gui;
pub mod component;
pub mod wm;
pub mod term;
pub mod compose;
pub mod sys;
```

- [ ] **Step 3: Verify it compiles (link check happens in Task 3 after registration)**

The module is unused until Task 2 wires it; a standalone build now would warn "unused". Proceed to Task 2 before building.

- [ ] **Step 4: Commit**

```bash
cd /mnt/w/Work/GitHub/ruos
git add kernel/src/wasm/wt/sys.rs kernel/src/wasm/wt/mod.rs
git commit -m "feat(wt): sys host module — cpustat/proc_stat/meminfo/uptime for compositor windows"
```

---

### Task 2: Register `sys` on the compositor linkers

**Files:**
- Modify: `kernel/src/wasm/wt/wm.rs` (4 sites that build a window `Linker`)

Every site that calls `crate::wasm::wt::term::add_to_linker(&mut linker)` must also
call `sys::add_to_linker`. There are exactly four (lines 244, 863, 986, 1027).

- [ ] **Step 1: `scan_apps` (around line 244)**

Find:

```rust
    if crate::wasm::wt::term::add_to_linker(&mut linker).is_err() { return; }
```

Add immediately after:

```rust
    if crate::wasm::wt::sys::add_to_linker(&mut linker).is_err() { return; }
```

- [ ] **Step 2: `run_reactor_spike` (around line 863)**

Find:

```rust
    if crate::wasm::wt::term::add_to_linker(&mut linker).is_err() { return (0, 0, 0); }
```

Add immediately after:

```rust
    if crate::wasm::wt::sys::add_to_linker(&mut linker).is_err() { return (0, 0, 0); }
```

- [ ] **Step 3: `Compositor::new` (around line 986)**

Find:

```rust
        crate::wasm::wt::term::add_to_linker(&mut linker).expect("term linker");
```

Add immediately after:

```rust
        crate::wasm::wt::sys::add_to_linker(&mut linker).expect("sys linker");
```

- [ ] **Step 4: `new_empty` (around line 1027)**

Find (the second occurrence of the `term linker` expect, inside `new_empty`):

```rust
        crate::wasm::wt::term::add_to_linker(&mut linker).expect("term linker");
```

Add immediately after:

```rust
        crate::wasm::wt::sys::add_to_linker(&mut linker).expect("sys linker");
```

- [ ] **Step 5: Commit**

```bash
cd /mnt/w/Work/GitHub/ruos
git add kernel/src/wasm/wt/wm.rs
git commit -m "feat(wt): register sys host module on the compositor window linkers"
```

---

### Task 3: Build the kernel (link check)

**Files:** none (build only).

- [ ] **Step 1: Headless build + boot test via WSL**

Run:

```bash
wsl -d Ubuntu-22.04 -u root -e bash -lc "cd /mnt/w/Work/GitHub/ruos && make run-test"
```

Expected: builds without errors and prints the boot success string (the `HELLO`
marker in the Makefile). A compile error in `sys.rs` (e.g. a wrong kernel API path)
surfaces here. Fix and rebuild until green.

- [ ] **Step 2: Add CHANGELOG entry**

Check the current max number: `ls CHANGELOG | sort -t- -k1 -n | tail -1` (expect ≥
338 on this branch). Create `CHANGELOG/<next>-26-06-08-wt-sys-host-module.md`:

```markdown
# <next> — Host module `sys` per le finestre del compositor

**Data:** 2026-06-08

## Cosa
Nuovo modulo host Wasmtime `kernel/src/wasm/wt/sys.rs` (`sys.cpustat`/`proc_stat`/
`meminfo`/`uptime`), registrato sui linker delle finestre in `wm.rs`. Espone alla
GUI gli stessi blob che rtop legge via il modulo wasmi `ruos`.

## Perché
Il System Monitor (finestra egui su Wasmtime) deve leggere dati reali; gli host fn
`ruos` esistenti stanno solo sul linker wasmi.

## File toccati
- kernel/src/wasm/wt/sys.rs
- kernel/src/wasm/wt/mod.rs
- kernel/src/wasm/wt/wm.rs
- CHANGELOG/<next>-26-06-08-wt-sys-host-module.md
```

- [ ] **Step 3: Commit**

```bash
cd /mnt/w/Work/GitHub/ruos
git add CHANGELOG/
git commit -m "docs(changelog): wt sys host module"
```

---

## Repo B — submodule (ruos-desktop)

> All Repo B `cargo` commands run on the **Windows host** (PowerShell), per
> `ruos-desktop/CLAUDE.md` ("non serve WSL qui"). Working dir:
> `W:\Work\GitHub\ruos\ruos-desktop`.

### Task 4: gui-core — `SysInfoSource` trait, POD snapshots, pure diff fns (TDD)

**Files:**
- Create: `ruos-desktop/crates/gui-core/src/desktop/apps/sysinfo.rs`
- Modify: `ruos-desktop/crates/gui-core/src/desktop/apps/mod.rs` (declare module)

- [ ] **Step 1: Create the submodule branch**

```powershell
cd W:\Work\GitHub\ruos\ruos-desktop
git checkout -b feat/system-monitor-real-data
git branch --show-current
```

Expected: `feat/system-monitor-real-data`.

- [ ] **Step 2: Write the failing test (pure diff fns + types)**

Create `crates/gui-core/src/desktop/apps/sysinfo.rs`:

```rust
//! System telemetry capability + POD snapshots for the System Monitor. PURE: this
//! defines the data the window renders and the math to diff two samples; it does
//! NOT fetch (the host-fn-backed impl lives in `ruos-window`, on-device only). The
//! kernel exposes cumulative `cpu_tsc` + per-core busy/idle, so per-process CPU%
//! and per-core utilisation come from differencing two snapshots over wall time.

/// Per-core cumulative busy/idle TSC counters (monotonic).
#[derive(Clone, Copy, Default)]
pub struct CoreLoad {
    pub busy: u64,
    pub idle: u64,
}

/// A CPU snapshot: the calibrated TSC frequency + per-core counters.
#[derive(Clone, Default)]
pub struct CpuSnapshot {
    pub tsc_per_ms: u64,
    pub cores: Vec<CoreLoad>,
}

/// One process row (cumulative cpu_tsc + last-observed linear-memory bytes).
#[derive(Clone)]
pub struct ProcRow {
    pub pid: u32,
    pub name: String,
    pub cpu_tsc: u64,
    pub mem_bytes: u64,
}

/// Memory snapshot: kernel heap + physical frames (frames are 4 KiB pages).
#[derive(Clone, Copy, Default)]
pub struct MemSnapshot {
    pub heap_total: u64,
    pub heap_used: u64,
    pub frames_total: u64,
    pub frames_used: u64,
}

/// The capability the System Monitor pulls telemetry from. Implemented by
/// `ruos-window::RuosSysInfo` on-device; a test fake implements it for unit tests.
pub trait SysInfoSource {
    fn cpustat(&self) -> CpuSnapshot;
    fn procs(&self) -> Vec<ProcRow>;
    fn meminfo(&self) -> MemSnapshot;
    /// Monotonic uptime in centiseconds (the wall clock used to diff samples).
    fn uptime_cs(&self) -> u64;
}

/// Per-process CPU% from two cumulative `cpu_tsc` samples over `dwall_ms`
/// milliseconds. `prev = None` (first sample) or a non-positive interval → 0.
pub fn proc_cpu_pct(prev_tsc: Option<u64>, cur_tsc: u64, tsc_per_ms: u64, dwall_ms: f64) -> f32 {
    match prev_tsc {
        Some(p) if dwall_ms > 0.0 && tsc_per_ms > 0 => {
            let dcyc = cur_tsc.saturating_sub(p) as f64;
            let cpu_ms = dcyc / tsc_per_ms as f64;
            ((cpu_ms / dwall_ms) * 100.0) as f32
        }
        _ => 0.0,
    }
}

/// Per-core utilisation% = Δbusy / (Δbusy + Δidle). `prev = None` → 0.
pub fn core_util(prev: Option<(u64, u64)>, cur: (u64, u64)) -> f32 {
    match prev {
        Some((pb, pi)) => {
            let db = cur.0.saturating_sub(pb) as f64;
            let di = cur.1.saturating_sub(pi) as f64;
            let tot = db + di;
            if tot > 0.0 { (db / tot * 100.0) as f32 } else { 0.0 }
        }
        None => 0.0,
    }
}

/// Aggregate CPU% across all cores = ΣΔbusy / Σ(Δbusy + Δidle). `have_prev=false`
/// (no previous sample) → 0. Cores absent from `prev` are skipped.
pub fn aggregate_util(prev: &[CoreLoad], cur: &[CoreLoad], have_prev: bool) -> f32 {
    if !have_prev { return 0.0; }
    let mut db = 0.0f64;
    let mut di = 0.0f64;
    for (i, c) in cur.iter().enumerate() {
        if let Some(p) = prev.get(i) {
            db += c.busy.saturating_sub(p.busy) as f64;
            di += c.idle.saturating_sub(p.idle) as f64;
        }
    }
    let tot = db + di;
    if tot > 0.0 { (db / tot * 100.0) as f32 } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_cpu_pct_first_sample_is_zero() {
        assert_eq!(proc_cpu_pct(None, 1_000_000, 1000, 100.0), 0.0);
    }

    #[test]
    fn proc_cpu_pct_half_a_core() {
        // tsc_per_ms = 1000 → 1 ms of CPU = 1000 cycles. Over a 200 ms window the
        // process burned 100 ms of CPU (100_000 cycles) → 50%.
        let pct = proc_cpu_pct(Some(0), 100_000, 1000, 200.0);
        assert!((pct - 50.0).abs() < 0.01, "got {pct}");
    }

    #[test]
    fn proc_cpu_pct_zero_interval_guard() {
        assert_eq!(proc_cpu_pct(Some(0), 100_000, 1000, 0.0), 0.0);
    }

    #[test]
    fn core_util_busy_three_quarters() {
        // Δbusy = 300, Δidle = 100 → 75%.
        let u = core_util(Some((0, 0)), (300, 100));
        assert!((u - 75.0).abs() < 0.01, "got {u}");
    }

    #[test]
    fn aggregate_util_sums_cores() {
        let prev = vec![CoreLoad { busy: 0, idle: 0 }, CoreLoad { busy: 0, idle: 0 }];
        let cur = vec![CoreLoad { busy: 100, idle: 100 }, CoreLoad { busy: 300, idle: 100 }];
        // ΣΔbusy = 400, ΣΔidle = 200 → 400/600 = 66.66%.
        let u = aggregate_util(&prev, &cur, true);
        assert!((u - 66.666).abs() < 0.1, "got {u}");
    }
}
```

Declare the module — in `crates/gui-core/src/desktop/apps/mod.rs`, add `pub mod sysinfo;` after the existing `pub mod system;` line:

```rust
pub mod about;
pub mod files;
pub mod notepad;
pub mod system;
pub mod sysinfo;
pub mod term;
pub mod terminal;
```

- [ ] **Step 3: Run the test to verify it fails to compile / passes math**

```powershell
cd W:\Work\GitHub\ruos\ruos-desktop
cargo test -p gui-core sysinfo
```

Expected: the `sysinfo::tests` compile and PASS (the functions are implemented in
this same step; this is a self-contained TDD unit — the assertions lock the math).

- [ ] **Step 4: Commit**

```powershell
cd W:\Work\GitHub\ruos\ruos-desktop
git add crates/gui-core/src/desktop/apps/sysinfo.rs crates/gui-core/src/desktop/apps/mod.rs
git commit -m "feat(gui-core): SysInfoSource trait + POD snapshots + diff math (tested)"
```

---

### Task 5: gui-core — rewrite `System` to render real snapshots

**Files:**
- Modify (full replace): `ruos-desktop/crates/gui-core/src/desktop/apps/system.rs`

- [ ] **Step 1: Replace the entire file**

Overwrite `crates/gui-core/src/desktop/apps/system.rs` with:

```rust
//! System Monitor — real kernel telemetry (CPU / memory / processes) rendered from
//! a `SysInfoSource` capability. gui-core stays pure: it renders snapshots, it does
//! not call host fns (the host-backed source lives in `ruos-window`; on PC the app
//! is not shown). Per-process CPU% and per-core utilisation come from diffing two
//! samples over wall time (the kernel exposes only cumulative `cpu_tsc` + busy/idle).

use crate::desktop::app_trait::DeskApp;
use crate::desktop::apps::sysinfo::{
    aggregate_util, core_util, proc_cpu_pct, CoreLoad, MemSnapshot, SysInfoSource,
};
use egui_extras::{Column, TableBuilder};
use std::collections::BTreeMap;

/// Samples kept in the total-CPU history graph.
const HISTORY: usize = 150;
/// Resample period in centiseconds (20 cs = 200 ms ≈ 5 Hz). Diffing every frame
/// would be noisy; this bounds the diff window and the `proc::list` clone cost.
const SAMPLE_CS: u64 = 20;

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Cpu,
    Memory,
}

#[derive(PartialEq, Clone, Copy)]
enum SortBy {
    Cpu,
    CpuTime,
    Pid,
    Memory,
    Name,
}

/// A process row prepared for display (CPU% diffed, CPU time in seconds).
struct ProcDisplay {
    pid: u32,
    name: String,
    cpu_pct: f32,
    cpu_secs: f64,
    mem_bytes: u64,
}

const C_BUSY: egui::Color32 = egui::Color32::from_rgb(52, 120, 246); // blu
const C_IDLE: egui::Color32 = egui::Color32::from_gray(150);

/// System Monitor: pulls telemetry from a `SysInfoSource`, diffs samples, renders.
pub struct System {
    src: Box<dyn SysInfoSource>,
    tab: Tab,
    sort: SortBy,
    sort_desc: bool,

    // diff state (previous sample)
    prev_proc: BTreeMap<u32, u64>, // pid -> cpu_tsc
    prev_cores: Vec<CoreLoad>,
    prev_cs: u64,
    have_prev: bool,

    // latest computed view
    rows: Vec<ProcDisplay>,
    core_util: Vec<f32>,
    total_hist: Vec<f32>,
    mem: MemSnapshot,
    uptime_cs: u64,
}

// Palette.
const C_BAR_BG: egui::Color32 = egui::Color32::from_rgba_premultiplied(12, 14, 20, 120);

impl System {
    /// Build the monitor over a telemetry source (the real one on-device; a fake in
    /// tests). There is no `Default` — a source is required.
    pub fn new(src: Box<dyn SysInfoSource>) -> Self {
        Self {
            src,
            tab: Tab::Cpu,
            sort: SortBy::Cpu,
            sort_desc: true,
            prev_proc: BTreeMap::new(),
            prev_cores: Vec::new(),
            prev_cs: 0,
            have_prev: false,
            rows: Vec::new(),
            core_util: Vec::new(),
            total_hist: vec![0.0; HISTORY],
            mem: MemSnapshot::default(),
            uptime_cs: 0,
        }
    }

    /// Resample at most ~5 Hz: read the cheap uptime clock; if a period elapsed (or
    /// first call), pull all snapshots and recompute the view by diffing.
    fn maybe_sample(&mut self) {
        let now = self.src.uptime_cs();
        if self.have_prev && now.saturating_sub(self.prev_cs) < SAMPLE_CS {
            return;
        }
        self.sample(now);
    }

    /// One sample at uptime `now` (centiseconds): compute per-core util, per-proc
    /// CPU%, total% history, then advance the previous-sample state.
    fn sample(&mut self, now: u64) {
        let cpu = self.src.cpustat();
        let procs = self.src.procs();
        self.mem = self.src.meminfo();
        self.uptime_cs = now;

        let dwall_ms = if self.have_prev {
            now.saturating_sub(self.prev_cs) as f64 * 10.0 // cs → ms
        } else {
            0.0
        };

        // Per-core utilisation.
        self.core_util = cpu
            .cores
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let prev = self.prev_cores.get(i).map(|p| (p.busy, p.idle));
                core_util(prev, (c.busy, c.idle))
            })
            .collect();

        // Per-process CPU% + display rows.
        let mut rows = Vec::with_capacity(procs.len());
        for p in &procs {
            let prev = if self.have_prev { self.prev_proc.get(&p.pid).copied() } else { None };
            let pct = proc_cpu_pct(prev, p.cpu_tsc, cpu.tsc_per_ms, dwall_ms);
            let cpu_secs = if cpu.tsc_per_ms > 0 {
                p.cpu_tsc as f64 / cpu.tsc_per_ms as f64 / 1000.0
            } else {
                0.0
            };
            rows.push(ProcDisplay {
                pid: p.pid,
                name: p.name.clone(),
                cpu_pct: pct,
                cpu_secs,
                mem_bytes: p.mem_bytes,
            });
        }
        self.rows = rows;

        // Total CPU% (aggregate across cores) into the history ring.
        let agg = aggregate_util(&self.prev_cores, &cpu.cores, self.have_prev);
        if self.have_prev {
            self.total_hist.remove(0);
            self.total_hist.push(agg);
        }

        // Advance previous-sample state.
        self.prev_proc = procs.iter().map(|p| (p.pid, p.cpu_tsc)).collect();
        self.prev_cores = cpu.cores.clone();
        self.prev_cs = now;
        self.have_prev = true;
    }
}

impl DeskApp for System {
    fn id(&self) -> &'static str {
        "system"
    }
    fn title(&self) -> &'static str {
        "System Monitor"
    }
    fn ui(&mut self, ui: &mut egui::Ui) {
        self.maybe_sample();
        self.tab_bar(ui);
        ui.add_space(8.0);
        match self.tab {
            Tab::Cpu => self.cpu_tab(ui),
            Tab::Memory => self.memory_tab(ui),
        }
        // Keep refreshing even without input (the data ticks on its own clock).
        ui.ctx().request_repaint();
    }
}

/// Translucent "glass" panel, macOS vibrancy style.
fn glass<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(egui::Color32::from_rgba_unmultiplied(30, 32, 40, 150))
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 18)))
        .corner_radius(8)
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| add(ui))
        .inner
}

fn mib(bytes: u64) -> f32 {
    bytes as f32 / (1024.0 * 1024.0)
}

impl System {
    fn tab_bar(&mut self, ui: &mut egui::Ui) {
        glass(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (tab, label) in [(Tab::Cpu, "CPU"), (Tab::Memory, "Memoria")] {
                    if ui.selectable_label(self.tab == tab, format!("  {label}  ")).clicked() {
                        self.tab = tab;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(format!("uptime {}s", self.uptime_cs / 100));
                });
            });
        });
    }

    /// CPU tab: per-core utilisation bars + total history graph + process table.
    fn cpu_tab(&mut self, ui: &mut egui::Ui) {
        glass(ui, |ui| {
            ui.heading("CPU");
            ui.add_space(4.0);
            for (i, &u) in self.core_util.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.add_sized([60.0, 16.0], egui::Label::new(format!("Core {i}")));
                    ui.add(
                        egui::ProgressBar::new((u / 100.0).clamp(0.0, 1.0))
                            .desired_width(ui.available_width() - 70.0)
                            .fill(C_BUSY.gamma_multiply(0.9))
                            .text(format!("{u:.0}%")),
                    );
                });
                ui.add_space(2.0);
            }
            self.cpu_graph(ui);
        });

        ui.add_space(8.0);
        self.proc_table(ui);
    }

    /// Total-CPU% history graph (single series, aggregate across cores).
    fn cpu_graph(&self, ui: &mut egui::Ui) {
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 70.0), egui::Sense::hover());
        let painter = ui.painter().with_clip_rect(rect);
        painter.rect_filled(rect, 6.0, C_BAR_BG);

        let grid = egui::Color32::from_rgba_unmultiplied(255, 255, 255, 20);
        for k in 1..4 {
            let y = rect.top() + rect.height() * (k as f32 / 4.0);
            painter.line_segment(
                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                egui::Stroke::new(1.0, grid),
            );
        }

        let n = self.total_hist.len();
        let dx = rect.width() / (n.max(2) - 1) as f32;
        let y_at = |v: f32| rect.bottom() - (v / 100.0).clamp(0.0, 1.0) * rect.height();
        let pts: Vec<egui::Pos2> = (0..n)
            .map(|i| egui::pos2(rect.left() + dx * i as f32, y_at(self.total_hist[i])))
            .collect();
        painter.add(egui::Shape::line(pts, egui::Stroke::new(1.4, C_BUSY)));

        let last = self.total_hist.last().copied().unwrap_or(0.0);
        painter.text(
            rect.left_top() + egui::vec2(6.0, 4.0),
            egui::Align2::LEFT_TOP,
            format!("CPU {last:.0}%  (scala 100%)"),
            egui::FontId::monospace(10.0),
            egui::Color32::from_gray(170),
        );
    }

    /// Sortable process table: Processo | % CPU | Tempo CPU | Memoria | PID.
    fn proc_table(&mut self, ui: &mut egui::Ui) {
        // Build the display order per the sort column.
        let mut order: Vec<usize> = (0..self.rows.len()).collect();
        let sort = self.sort;
        let rows = &self.rows;
        order.sort_by(|&a, &b| {
            let (ra, rb) = (&rows[a], &rows[b]);
            let ord = match sort {
                SortBy::Cpu => ra.cpu_pct.partial_cmp(&rb.cpu_pct).unwrap_or(core::cmp::Ordering::Equal),
                SortBy::CpuTime => ra.cpu_secs.partial_cmp(&rb.cpu_secs).unwrap_or(core::cmp::Ordering::Equal),
                SortBy::Pid => ra.pid.cmp(&rb.pid),
                SortBy::Memory => ra.mem_bytes.cmp(&rb.mem_bytes),
                SortBy::Name => ra.name.cmp(&rb.name),
            };
            if self.sort_desc { ord.reverse() } else { ord }
        });

        let mut clicked: Option<SortBy> = None;
        let arrow = |active: bool, desc: bool| if !active { "" } else if desc { " ▾" } else { " ▴" };
        fn hcol(ui: &mut egui::Ui, text: String) -> bool {
            ui.add(egui::Label::new(egui::RichText::new(text).strong()).sense(egui::Sense::click()))
                .clicked()
        }

        let table_h = (ui.available_height() - 20.0).max(80.0);
        glass(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(table_h)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    TableBuilder::new(ui)
                        .striped(true)
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                        .column(Column::remainder().at_least(100.0))
                        .column(Column::exact(70.0))
                        .column(Column::exact(90.0))
                        .column(Column::exact(90.0))
                        .column(Column::exact(60.0))
                        .header(20.0, |mut h| {
                            h.col(|ui| {
                                if hcol(ui, format!("Processo{}", arrow(self.sort == SortBy::Name, self.sort_desc))) {
                                    clicked = Some(SortBy::Name);
                                }
                            });
                            h.col(|ui| {
                                if hcol(ui, format!("% CPU{}", arrow(self.sort == SortBy::Cpu, self.sort_desc))) {
                                    clicked = Some(SortBy::Cpu);
                                }
                            });
                            h.col(|ui| {
                                if hcol(ui, format!("Tempo CPU{}", arrow(self.sort == SortBy::CpuTime, self.sort_desc))) {
                                    clicked = Some(SortBy::CpuTime);
                                }
                            });
                            h.col(|ui| {
                                if hcol(ui, format!("Memoria{}", arrow(self.sort == SortBy::Memory, self.sort_desc))) {
                                    clicked = Some(SortBy::Memory);
                                }
                            });
                            h.col(|ui| {
                                if hcol(ui, format!("PID{}", arrow(self.sort == SortBy::Pid, self.sort_desc))) {
                                    clicked = Some(SortBy::Pid);
                                }
                            });
                        })
                        .body(|mut body| {
                            for &i in &order {
                                let r = &rows[i];
                                body.row(18.0, |mut row| {
                                    row.col(|ui| { ui.label(&r.name); });
                                    row.col(|ui| { ui.monospace(format!("{:.1}", r.cpu_pct)); });
                                    row.col(|ui| { ui.monospace(format!("{:.1} s", r.cpu_secs)); });
                                    row.col(|ui| { ui.monospace(format!("{:.1} MiB", mib(r.mem_bytes))); });
                                    row.col(|ui| { ui.monospace(format!("{}", r.pid)); });
                                });
                            }
                        });
                });
        });

        if let Some(c) = clicked {
            if self.sort == c {
                self.sort_desc = !self.sort_desc;
            } else {
                self.sort = c;
                self.sort_desc = true;
            }
        }
    }

    /// Memory tab: kernel heap + physical frames, both real.
    fn memory_tab(&self, ui: &mut egui::Ui) {
        let m = self.mem;
        let page = 4096u64;
        let frames_used_b = m.frames_used * page;
        let frames_total_b = m.frames_total * page;
        glass(ui, |ui| {
            ui.heading("Memoria fisica (frame)");
            ui.add_space(6.0);
            let frac = if frames_total_b > 0 {
                frames_used_b as f32 / frames_total_b as f32
            } else {
                0.0
            };
            ui.add(egui::ProgressBar::new(frac).fill(C_BUSY.gamma_multiply(0.9)).text(format!(
                "{:.0} / {:.0} MiB",
                mib(frames_used_b),
                mib(frames_total_b)
            )));
            ui.weak(format!(
                "Libera: {:.0} MiB ({} / {} pagine)",
                mib(frames_total_b.saturating_sub(frames_used_b)),
                m.frames_used,
                m.frames_total
            ));
        });
        ui.add_space(8.0);
        glass(ui, |ui| {
            ui.heading("Heap kernel");
            ui.add_space(6.0);
            // heap_used is 0 (talc has no used-bytes API) → show the reservation.
            ui.weak(format!("Riservato: {:.0} MiB", mib(self.mem.heap_total)));
            ui.add_space(2.0);
            ui.label(egui::RichText::new("Uso heap non disponibile (talc)").color(C_IDLE));
        });
    }
}
```

- [ ] **Step 2: Build gui-core (it won't fully build until Task 6 removes the old `default_apps` use)**

This step's file alone breaks `apps/mod.rs` (which still calls `System::default()`).
That's fixed in Task 6; do not build yet. Proceed to Task 6.

- [ ] **Step 3: Commit**

```powershell
cd W:\Work\GitHub\ruos\ruos-desktop
git add crates/gui-core/src/desktop/apps/system.rs
git commit -m "feat(gui-core): System renders real telemetry from SysInfoSource (no sim)"
```

---

### Task 6: gui-core — drop System from the PC preview + build/test

**Files:**
- Modify: `ruos-desktop/crates/gui-core/src/desktop/apps/mod.rs`

- [ ] **Step 1: Remove the `System` entry from `default_apps`**

In `crates/gui-core/src/desktop/apps/mod.rs`, the `default_apps()` body currently is:

```rust
pub fn default_apps() -> Vec<Box<dyn DeskApp>> {
    vec![
        Box::new(about::AboutRuos),
        Box::new(terminal::Terminal::default()),
        Box::new(files::Files),
        Box::new(system::System::default()),
        Box::new(notepad::Notepad::default()),
    ]
}
```

Remove the `system::System::default()` line so the PC monolithic preview no longer
builds System (it needs kernel data; no simulated source is shipped):

```rust
pub fn default_apps() -> Vec<Box<dyn DeskApp>> {
    vec![
        Box::new(about::AboutRuos),
        Box::new(terminal::Terminal::default()),
        Box::new(files::Files),
        Box::new(notepad::Notepad::default()),
    ]
}
```

(Keep `pub mod system;` and `pub mod sysinfo;` — `system-app` still imports the type.)

- [ ] **Step 2: Build gui-core + run all its tests**

```powershell
cd W:\Work\GitHub\ruos\ruos-desktop
cargo build -p gui-core
cargo test -p gui-core
```

Expected: builds clean; all tests pass (including `sysinfo::tests` from Task 4). If
the compiler flags `System::default` still referenced, grep for stragglers:
`Select-String -Path crates,backends,apps -Pattern "System::default" -Recurse`.

- [ ] **Step 3: Build the PC preview (sanity — gui-core must stay PC-buildable)**

```powershell
cd W:\Work\GitHub\ruos\ruos-desktop
cargo build -p pc-backend
```

Expected: builds clean (the preview simply no longer lists System).

- [ ] **Step 4: Commit**

```powershell
cd W:\Work\GitHub\ruos\ruos-desktop
git add crates/gui-core/src/desktop/apps/mod.rs
git commit -m "refactor(gui-core): drop System from PC preview (needs kernel data)"
```

---

### Task 7: ruos-window — `RuosSysInfo` (host-fn-backed source)

**Files:**
- Modify: `ruos-desktop/crates/ruos-window/src/lib.rs` (add `sys` extern module + impl)

- [ ] **Step 1: Add the `sys` host bindings**

In `crates/ruos-window/src/lib.rs`, after the `mod term { ... }` block (ends ~line 54),
add a new extern module:

```rust
// The `sys` host module (kernel/src/wasm/wt/sys.rs `add_to_linker`): live kernel
// telemetry (CPU / processes / memory / uptime) for the System Monitor. Same blob
// layouts as the wasmi `ruos` module rtop uses.
mod sys {
    #[link(wasm_import_module = "sys")]
    extern "C" {
        pub fn cpustat(ptr: *mut u8, len: u32) -> i32;
        pub fn proc_stat(ptr: *mut u8, len: u32, used: *mut u32) -> i32;
        pub fn meminfo(ptr: *mut u8) -> i32;
        pub fn uptime() -> i64;
    }
}
```

- [ ] **Step 2: Add `RuosSysInfo` + blob parsers**

At the end of `crates/ruos-window/src/lib.rs`, add:

```rust
use gui_core::desktop::apps::sysinfo::{
    CoreLoad, CpuSnapshot, MemSnapshot, ProcRow, SysInfoSource,
};

fn rd_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn rd_u64(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}

/// `SysInfoSource` over the `sys` host module. The System Monitor app constructs
/// one and hands it to `gui_core::desktop::apps::system::System::new`.
pub struct RuosSysInfo;

impl SysInfoSource for RuosSysInfo {
    fn cpustat(&self) -> CpuSnapshot {
        // u32 ncores, u64 tsc_per_ms, then ncores × (u64 busy, u64 idle).
        // 16 cores fit in 4 + 8 + 16*16 = 268 bytes; 512 is generous.
        let mut buf = [0u8; 512];
        let rc = unsafe { sys::cpustat(buf.as_mut_ptr(), buf.len() as u32) };
        if rc != 0 {
            return CpuSnapshot::default();
        }
        if buf.len() < 12 {
            return CpuSnapshot::default();
        }
        let ncores = rd_u32(&buf, 0) as usize;
        let tsc_per_ms = rd_u64(&buf, 4);
        let mut cores = Vec::with_capacity(ncores);
        let mut o = 12;
        for _ in 0..ncores {
            if o + 16 > buf.len() {
                break;
            }
            cores.push(CoreLoad { busy: rd_u64(&buf, o), idle: rd_u64(&buf, o + 8) });
            o += 16;
        }
        CpuSnapshot { tsc_per_ms, cores }
    }

    fn procs(&self) -> Vec<ProcRow> {
        // u32 count, then per row: u32 pid, u64 start_tick, u64 cpu_tsc,
        // u64 mem_bytes, u16 name_len, u16 pad, name. 16 KiB holds ~250 short rows.
        let mut buf = vec![0u8; 16 * 1024];
        let mut used: u32 = 0;
        let rc = unsafe { sys::proc_stat(buf.as_mut_ptr(), buf.len() as u32, &mut used) };
        if rc != 0 || buf.len() < 4 {
            return Vec::new();
        }
        let avail = (used as usize).min(buf.len()); // parse only validly-written bytes
        let count = rd_u32(&buf, 0) as usize;
        let mut out = Vec::with_capacity(count);
        let mut o = 4;
        for _ in 0..count {
            if o + 30 > avail {
                break; // header (4+8+8+8+2+2 = 32) won't fit → truncated tail
            }
            let pid = rd_u32(&buf, o);
            // start_tick @ o+4 (u64) unused here.
            let cpu_tsc = rd_u64(&buf, o + 12);
            let mem_bytes = rd_u64(&buf, o + 20);
            let name_len = rd_u16(&buf, o + 28) as usize;
            o += 32; // pid+start_tick+cpu_tsc+mem_bytes+name_len+pad
            if o + name_len > avail {
                break;
            }
            let name = String::from_utf8_lossy(&buf[o..o + name_len]).into_owned();
            o += name_len;
            out.push(ProcRow { pid, name, cpu_tsc, mem_bytes });
        }
        out
    }

    fn meminfo(&self) -> MemSnapshot {
        // 4 × u64: heap_total, heap_used, frames_total, frames_used.
        let mut buf = [0u8; 32];
        let rc = unsafe { sys::meminfo(buf.as_mut_ptr()) };
        if rc != 0 {
            return MemSnapshot::default();
        }
        MemSnapshot {
            heap_total: rd_u64(&buf, 0),
            heap_used: rd_u64(&buf, 8),
            frames_total: rd_u64(&buf, 16),
            frames_used: rd_u64(&buf, 24),
        }
    }

    fn uptime_cs(&self) -> u64 {
        let v = unsafe { sys::uptime() };
        if v < 0 {
            0
        } else {
            v as u64
        }
    }
}
```

> **Row stride note:** the per-row fixed prefix is `4 (pid) + 8 (start_tick) +
> 8 (cpu_tsc) + 8 (mem_bytes) + 2 (name_len) + 2 (pad) = 32` bytes, then `name_len`
> name bytes. The parser advances `o` by 32 then `name_len`. The `o + 30 > avail`
> guard ensures the `name_len` field (last 2 bytes of the prefix, at `o+28..o+30`)
> is readable before use.

- [ ] **Step 3: Build ruos-window for wasm**

```powershell
cd W:\Work\GitHub\ruos\ruos-desktop
cargo build -p ruos-window --target wasm32-wasip1
```

Expected: builds clean (the `extern "C"` `sys` imports resolve at link time in the
kernel, not here).

- [ ] **Step 4: Commit**

```powershell
cd W:\Work\GitHub\ruos\ruos-desktop
git add crates/ruos-window/src/lib.rs
git commit -m "feat(ruos-window): RuosSysInfo — SysInfoSource over the sys host module"
```

---

### Task 8: system-app — wire the real source

**Files:**
- Modify: `ruos-desktop/apps/system-app/src/lib.rs`

- [ ] **Step 1: Construct `System` with `RuosSysInfo`**

In `apps/system-app/src/lib.rs`, change the `frame()` body. Replace:

```rust
        if APP.is_none() {
            APP = Some(System::default());
        }
```

with:

```rust
        if APP.is_none() {
            APP = Some(System::new(Box::new(ruos_window::RuosSysInfo)));
        }
```

The module-doc comment at the top still says "simulated data" — update it to:

```rust
//! System Monitor window — a thin app on the `ruos-window` SDK wrapping gui-core's
//! `System` DeskApp (process table + per-core CPU + memory, REAL kernel telemetry
//! via `ruos_window::RuosSysInfo` over the `sys` host module). `frame()` drives the
//! SDK; the closure renders the app's `ui` in a CentralPanel under the CSD title
//! bar. Larger surface (720×520) so the table + charts have room. A `cdylib`
//! reactor: the wasm `#[no_mangle] frame`/`_start` exports must live here; the
//! kernel calls `_initialize` once then `frame()` serially per window, so the
//! `static mut`s are single-threaded-safe.
```

- [ ] **Step 2: Build system-app to wasm**

```powershell
cd W:\Work\GitHub\ruos\ruos-desktop
cargo build -p system-app --target wasm32-wasip1 --release
```

Expected: builds clean → `target/wasm32-wasip1/release/system_app.wasm` (the
Makefile precompiles it to `system.cwasm`).

- [ ] **Step 3: Commit**

```powershell
cd W:\Work\GitHub\ruos\ruos-desktop
git add apps/system-app/src/lib.rs
git commit -m "feat(system-app): wire RuosSysInfo — real System Monitor data"
```

---

## Integration

### Task 9: Full build + visual verification + submodule bump

**Files:**
- Modify: `W:\Work\GitHub\ruos` (submodule pointer + CHANGELOG)

- [ ] **Step 1: Push the submodule branch (so the parent can point at it)**

Per the project rule, push from WSL with gh credentials:

```powershell
wsl -d Ubuntu-22.04 -e bash -lc "cd /mnt/w/Work/GitHub/ruos/ruos-desktop && git -c credential.helper='!gh auth git-credential' push -u origin feat/system-monitor-real-data"
```

- [ ] **Step 2: Full ISO build via WSL**

```powershell
wsl -d Ubuntu-22.04 -u root -e bash -lc "cd /mnt/w/Work/GitHub/ruos && make iso"
```

Expected: builds kernel (with `wt/sys.rs`) + the desktop crates + `system.cwasm` +
assembles the ISO with no errors. A blob/parse mismatch or a kernel API typo
surfaces here.

- [ ] **Step 3: Visual test (interactive QEMU)**

```powershell
wsl -d Ubuntu-22.04 -u root -e bash -lc "cd /mnt/w/Work/GitHub/ruos && make run"
```

Verify on screen:
- Desktop boots → launcher "☰ Apps" → click **System Monitor** → window opens.
- **CPU tab:** one "Core N" bar per online core, moving; the total-CPU% graph fills
  over a few seconds. Open another window / spawn a busy app → bars rise.
- **Process table:** real procs (`compositor`/`gui`, `win:system`, `win:*` for other
  open windows, `sshd` if up). `% CPU` is ~0 at rest and rises under load; `Tempo
  CPU` grows monotonically; `Memoria` non-zero. Click headers → sort flips.
- **Memoria tab:** physical-frames bar shows real used/total; spawning windows nudges
  it up. Heap shows "Riservato: 256 MiB" + the talc note.
- No **Temperatura** / **Energia** tab.

Serial log shows `wm.spawn ok name='system'`.

- [ ] **Step 4: Bump the submodule pointer + CHANGELOG (parent repo)**

```powershell
cd W:\Work\GitHub\ruos
git add ruos-desktop
```

Create `CHANGELOG/<next>-26-06-08-system-monitor-real-data.md` (next = current max +
1; this follows the kernel `sys` entry from Task 3):

```markdown
# <next> — System Monitor con dati reali (SP-F)

**Data:** 2026-06-08

## Cosa
La finestra System Monitor mostra telemetria reale del kernel invece dei dati
simulati: CPU per-core (busy/idle), processi (`proc::list`: pid/nome/CPU%/tempo
CPU/memoria), frame fisici, uptime. CPU% per processo calcolata diffando due
snapshot nel tempo. Rimosse le tab Temperatura ed Energia. Bump del submodule
`ruos-desktop` al branch `feat/system-monitor-real-data`.

## Perché
SP-E aveva spedito il System Monitor con dati placeholder; SP-F lo collega ai dati
veri tramite il nuovo host module `sys` e la capability `SysInfoSource`.

## File toccati
- ruos-desktop (submodule pointer)
- CHANGELOG/<next>-26-06-08-system-monitor-real-data.md
```

- [ ] **Step 5: Commit (parent)**

```powershell
cd W:\Work\GitHub\ruos
git add CHANGELOG/
git commit -m "feat(system-monitor): real kernel telemetry (SP-F) + submodule bump"
```

---

## Self-review notes (coverage map)

- Spec Part 1 (kernel `wt/sys.rs`) → Tasks 1-2; build check Task 3.
- Spec Part 2 (gui-core trait + POD + `System` refactor) → Tasks 4-6.
- Spec Part 3 (`RuosSysInfo`) → Task 7.
- Spec Part 4 (system-app wiring) → Task 8.
- Spec Part 5 (PC preview drop) → Task 6 Step 1.
- Spec testing (unit diff math, build, visual) → Task 4 (unit), Tasks 3/6/9 (build),
  Task 9 (visual).
- Decisions: Temperature/Energy removed (Task 5 — tabs gone); CPU% via diff (Task 4
  `proc_cpu_pct` + Task 5 `sample`); Threads→CPU time (Task 5 table); no sim source
  (Tasks 5-6). Blob layouts mirror rtop (Task 1 ≡ `host/sysinfo.rs`).
- Type consistency: `SysInfoSource`/`CpuSnapshot`/`CoreLoad`/`ProcRow`/`MemSnapshot`
  defined in Task 4, consumed verbatim in Tasks 5 and 7; `System::new(Box<dyn
  SysInfoSource>)` defined Task 5, called Task 8.
```
