# System Monitor real data (SP-F) — feed live CPU/mem/proc to the egui window (design)

**Date:** 2026-06-08
**Status:** approved (brainstorm), pending spec review → writing-plans
**Branches:** `feat/system-monitor-real-data` (ruos kernel) +
`<submodule branch>` (ruos-desktop: gui-core / ruos-window / system-app)

## Context — the arc

SP-E (`2026-06-05-egui-compositor-sp-e-apps-as-windows-design.md`) shipped the four
desktop apps (About / Files / Terminal / System Monitor) as compositor windows with
**placeholder/simulated data**, and explicitly deferred **real data** to SP-F: "a
kernel data host fn feeding real CPU/mem/`proc::list` to System Monitor (replacing
the simulation)". This is that SP-F, scoped to **System Monitor only**.

Today the `System` DeskApp (`ruos-desktop/crates/gui-core/src/desktop/apps/system.rs`)
is 100% simulated: process CPU% and sensor values are superposed sine waves
(`Proc::cpu(t)`, `Sensor::value(t)`), the process list is 8 hardcoded entries, heap
total is a hardcoded 128 MiB, and there are Temperature + Energy tabs with no real
backing.

Meanwhile **rtop** (`user/rtop/`) is a *real* system monitor — but a **wasmi** WASI
text tool. It reads live kernel state through four host fns in module `"ruos"`
(`cpustat`, `proc_stat`, `meminfo`, `uptime`), implemented in
`kernel/src/wasm/host/sysinfo.rs` for the **wasmi** runtime (`RuntimeState`,
`crate::wasm::host::mem`). The GUI runs on the **Wasmtime AOT** compositor path,
whose linker (built in `kernel/src/wasm/wt/wm.rs`) does NOT register those host fns.
So the data exists in the kernel but cannot reach the egui window.

## Goal

Open System Monitor from the desktop launcher → it shows **live, real** data:
- Per-core CPU utilisation (busy/idle) + a total-CPU history graph.
- Real process table: PID, name, CPU%, cumulative CPU time, memory — from
  `proc::list()`, sortable.
- Real memory: kernel heap used/total + physical frames used/total.
- Uptime.

No simulation on-device. Temperature and Energy tabs are **removed** (no thermal
sensors exist in the kernel; see Decisions). The PC preview (`pc-backend`
monolithic desktop) **drops** System Monitor (it requires kernel data; we ship no
simulated source — see Decisions).

## Decisions (brainstorm)

- **Architecture = capability trait (Approach A), mirroring `TermIo`.** `gui-core`
  defines a pure `SysInfoSource` trait + POD snapshot structs and renders from them;
  the host-fn-backed impl lives in `ruos-window`. `gui-core` stays pure (Golden
  Rule: only egui/tiny-skia — no host fns, no OS code). Rejected: gui-core calling
  host fns under `#[cfg]` (breaks the Golden Rule); passing data as a `ui()`
  argument (asymmetric — only System needs it, would change the `DeskApp` trait).
- **New Wasmtime host-fn module `kernel/src/wasm/wt/sys.rs`**, module name **`"sys"`**,
  registered on the compositor linker. Reuses the **same blob layouts** as rtop's
  `sysinfo.rs` so the marshalling logic mirrors the proven wasmi path.
- **Temperature + Energy tabs removed.** No DTS/ACPI thermal in the kernel; a
  placeholder tab adds surface without value. Monitor = CPU + Memory + Processes.
- **Per-process CPU% by diffing two snapshots over wall-time** (like rtop): the
  window keeps the previous `cpu_tsc` per pid and the previous wall time; first
  frame shows 0%.
- **Process table columns:** `Process | CPU% | CPU time | Memory | PID`. The old
  "Threads" column is dropped (wasm is single-threaded; fibers are the tasks) and
  replaced by **CPU time** (cumulative seconds from `cpu_tsc / tsc_per_ms`).
- **No simulated source shipped.** PC preview drops System (rather than carrying a
  sine-wave `SimSysInfo` just for PC).

## Architecture

```
gui-core (PURE)                      ruos-window (SDK)              kernel (ruos, Wasmtime path)
  SysInfoSource (trait)   ◄──impl──  RuosSysInfo  ──extern "C"──►   wt/sys.rs
  + POD snapshot types               (parse blobs)   module "sys"   add_to_linker<T>(linker)
  System (renders only)                                             reads live kernel state
        ▲ Box<dyn SysInfoSource>
        └── system-app: System::new(Box::new(RuosSysInfo))
```

### Part 1 — kernel: `kernel/src/wasm/wt/sys.rs` (new)

A Wasmtime linker module mirroring `host/sysinfo.rs` but for the `wt` path. Generic
over the store type `T` (no `HasWindow` bound — these fns read global kernel state
and only write guest memory):

```rust
pub fn add_to_linker<T>(linker: &mut Linker<T>) -> wasmtime::Result<()>;
```

registers module `"sys"` with:

| Host fn | Signature | Blob written to guest |
|---------|-----------|-----------------------|
| `cpustat`   | `(buf_ptr:i32, buf_len:i32) -> i32` | `ncores:u32, tsc_per_ms:u64, [busy:u64, idle:u64]×ncores` |
| `proc_stat` | `(buf_ptr:i32, buf_len:i32, used_ptr:i32) -> i32` | `count:u32, then [pid:u32, cpu_tsc:u64, mem_bytes:u64, name_len:u32, name:utf8]×count`; `used_ptr` ← bytes written |
| `meminfo`   | `(buf_ptr:i32) -> i32` | `heap_used:u64, heap_total:u64, frames_used:u64, frames_total:u64, page_size:u64` |
| `uptime`    | `() -> i64` | return value = centiseconds (`timer::ticks()`) |

- Blob layouts are **byte-identical** to rtop's where they overlap, so the
  guest-side parser is the same shape as `user/rtop/src/sys.rs`.
- Returns: `0` = ok, `8` = ERANGE (buffer too small — guest retries/ignores), an
  errno from `wt::mem::write` on EFAULT.
- Reads: `crate::sched::cpustat::read(cpu)`, `crate::cpu::cpus_online()`,
  `crate::boot::clock::tsc_per_ms()`, `crate::proc::list()`,
  `crate::memory::frame_counts()`, `crate::memory::HEAP_SIZE` (+ heap-used source),
  `crate::timer::ticks()`.
- **Heap used:** if `talc` exposes used bytes use it; otherwise report
  `heap_total` and `heap_used = 0` with a TODO (frames are the meaningful pressure
  metric anyway). Confirm during planning.
- **Registration:** call `crate::wasm::wt::sys::add_to_linker(&mut linker)?` at every
  site in `wm.rs` that builds a window linker — `Compositor::new`, `new_empty`
  (and, harmlessly, the scan/spike linkers) — right after the `wm`/`term` adds.

### Part 2 — gui-core: trait + POD types + `System` refactor (PURE)

New `desktop/apps/sysinfo.rs` (or a `sysinfo` mod inside `system.rs`):

```rust
pub struct CoreLoad { pub busy: u64, pub idle: u64 }
pub struct CpuSnapshot { pub tsc_per_ms: u64, pub cores: Vec<CoreLoad> }
pub struct ProcRow { pub pid: u32, pub name: String, pub cpu_tsc: u64, pub mem_bytes: u64 }
pub struct MemSnapshot {
    pub heap_used: u64, pub heap_total: u64,
    pub frames_used: u64, pub frames_total: u64, pub page_size: u64,
}
pub trait SysInfoSource {
    fn cpustat(&self) -> CpuSnapshot;
    fn procs(&self) -> Vec<ProcRow>;
    fn meminfo(&self) -> MemSnapshot;
    fn uptime_cs(&self) -> u64;
}
```

`System` refactor (`system.rs`):
- Holds `src: Box<dyn SysInfoSource>`, `prev_proc: BTreeMap<u32, u64>` (pid→cpu_tsc),
  `prev_cores: Vec<CoreLoad>`, `last_wall_ms: f64`, plus the real total-CPU% history
  ring (reuse the existing 150-sample buffer).
- **Sampling:** each frame (or throttled ~5–10 Hz) pull all four snapshots, compute:
  - per-core util = `Δbusy / (Δbusy + Δidle)`,
  - per-proc CPU% = `Δcpu_tsc / (tsc_per_ms × Δwall_ms) × 100`,
  - total CPU% = aggregate across cores `ΣΔbusy / Σ(Δbusy + Δidle)` (one series;
    the old user/system split had no real backing and is dropped); push to history.
  First frame: no `prev` ⇒ 0%.
- **Remove:** `Sensor`, `Proc::cpu` sine sim, `Tab::Temperature`, `Tab::Energy`,
  `temperature_tab`, the hardcoded proc/sensor vecs.
- **Tabs:** `Cpu` (per-core bars + total history graph), `Memory` (heap bar +
  frames bar, real), and the process table.
- **Process table:** `Process | CPU% | CPU time | Memory | PID`, sortable
  (`SortBy::{Cpu, CpuTime, Memory, Name, Pid}`). CPU time = `cpu_tsc / tsc_per_ms`
  → seconds. Memory = `mem_bytes`.
- gui-core ships **no** `SysInfoSource` impl (trait only) — purity preserved.

### Part 3 — ruos-window: `RuosSysInfo`

```rust
mod sys {                       // extern "C" to host module "sys"
    #[link(wasm_import_module = "sys")]
    extern "C" {
        pub fn cpustat(ptr: *mut u8, len: u32) -> i32;
        pub fn proc_stat(ptr: *mut u8, len: u32, used: *mut u32) -> i32;
        pub fn meminfo(ptr: *mut u8) -> i32;
        pub fn uptime() -> i64;
    }
}
pub struct RuosSysInfo;
impl gui_core::desktop::apps::sysinfo::SysInfoSource for RuosSysInfo { /* call + parse */ }
```

Fixed guest buffers (stack/`Vec`): cores ≤ 16, procs ≤ 64 (≈ `proc_stat` blob bound).
On `ERANGE`/error, return an empty snapshot for that metric (UI shows zeros).

### Part 4 — system-app

```rust
static mut APP: Option<System> = None;
// first frame:
APP = Some(System::new(Box::new(ruos_window::RuosSysInfo)));
```

`System::new(src: Box<dyn SysInfoSource>)` replaces `System::default()`. Window size
unchanged (720×520), manifest unchanged.

### Part 5 — PC preview

Remove System from the monolithic preview's app set (`default_apps()` / wherever
`Desktop`/`app.rs` constructs it). The monolithic preview no longer shows System
Monitor. (Verify the exact construction site during planning; `pc-backend` and
gui-core must still build with the System type present but unused by the preview.)

## Data flow

```
compositor frame() → System::ui
  → src.cpustat()/procs()/meminfo()/uptime_cs()
     on-device RuosSysInfo → sys.* host fns → kernel state → LE blob → parse
  → diff vs prev (cpu_tsc, cores) over Δwall → per-core util, per-proc CPU%, total%
  → egui per-core bars + history graph + sortable proc table + heap/frames bars
  → tessellate → wm.commit (commit-on-damage)
```

## Error handling

- Host fn: buffer too small → `8` (ERANGE); guest mem fault → errno from
  `wt::mem::write`. The kernel never traps on these (graceful).
- SDK: any non-zero return → empty snapshot for that call → UI renders zeros / "no
  data", never panics.
- First frame (no prev snapshot): all CPU% = 0; the history graph fills over time.
- `Δwall_ms == 0` guard: skip the CPU% update that frame (avoid div-by-zero).
- pid churn: a proc that vanished between snapshots drops from `prev` naturally
  (rebuild `prev` from the current list each sample).

## Testing / verification

1. **Unit (gui-core)** — a fake `SysInfoSource` returning scripted snapshots; assert
   the diff math: two `cpu_tsc` samples + a known `Δwall`/`tsc_per_ms` → expected
   CPU%; per-core util from busy/idle deltas; CPU-time seconds formatting. No kernel
   needed; runs on the host with `cargo test -p gui-core`.
2. **Build** — kernel (3 profiles) builds with `wt/sys.rs` linked; `system-app`
   builds to `system.cwasm`; gui-core + ruos-window + pc-backend build. Via WSL.
3. **Visual (QEMU/KVM)** — boot → desktop → launcher → System Monitor:
   - per-core CPU bars move under load (e.g. open another window / run a busy app);
   - process table lists real procs (`kernel`, `executor`, `compositor`/`gui`,
     `win:*`, `sshd` if up) with live CPU% + memory; sorting works;
   - Memory tab shows real heap + frames (frames climb when spawning windows);
   - uptime increases. Screendump.
4. **No Temperature/Energy tab** present.

## Risks / notes

- **Heap-used metric** may be unavailable from `talc` cheaply; frames are the real
  pressure signal. Fall back to frames-only if heap-used is hard (planning).
- **`cld` before guest calls:** the new host fns run inside guest calls already
  guarded by the compositor's `cld`; the marshalling uses safe slices, no manual
  `rep movs`.
- **Sampling cost:** `proc::list()` clones a `Vec<ProcInfo>` (Strings) each sample;
  throttle to ~5–10 Hz, not every frame, to bound cost. Commit-on-damage already
  limits redraw.
- **Two repos:** kernel host fn in `ruos`; gui-core/ruos-window/system-app in the
  `ruos-desktop` submodule. The submodule's Makefile coupling (per its CLAUDE.md)
  is unaffected (no crate moves/renames). Two commits; bump the submodule pointer in
  ruos after.
- **Module name `"sys"`** is distinct from wasmi's `"ruos"` — same data, different
  linker; no collision.

## Out of scope (SP-F)

- CPU temperature / thermal (no sensors).
- Per-thread breakdown, process tree, load average, disk/net I/O stats.
- Killing processes from the UI (`proc::request_kill` exists; a later step).
- Making rtop and System Monitor share a crate (separate runtimes; the blob layout
  is the shared contract, which is enough).
- A simulated PC source.

## Provides (later)

- The `SysInfoSource` capability + `wt/sys.rs` host module — reusable by any future
  compositor app needing kernel telemetry (a taskbar CPU meter, a process killer).
- The pattern: "kernel telemetry into an egui window via a gui-core capability trait
  + a ruos-window host-fn impl".
