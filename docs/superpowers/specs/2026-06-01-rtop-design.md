# rtop — htop-like system monitor for ruos — Design

**Date:** 2026-06-01
**Status:** approved (design), pending spec review
**Branch:** `feature/htop`

## Goal

A full-screen, self-refreshing system monitor (`rtop`) — ruos's `htop` —
runnable from the shell over the framebuffer console or SSH. Shows per-core
CPU%, system memory, uptime, task count, and a process table with per-process
CPU%, memory, cumulative CPU time, PID and command, sorted by CPU% descending.
Press `q` to quit.

## Why this is non-trivial on ruos

ruos is a **cooperative, single-CPU-userland, no-preemption** kernel (2026 pivot).
There is no preemptive scheduler, so there is no classic per-thread `utime`/`stime`
accounting. CPU usage must be derived from the cooperative execution model:

- At any instant **exactly one wasm fiber runs** on the BSP (the executor polls
  serially, then `sti; hlt` when idle). So **CPU time = TSC cycles spent
  executing**, and **idle time = TSC cycles spent halted**.
- A process's CPU time is the sum of the TSC deltas across its synchronous
  **wasmi run-bursts** (the cycles between two host-fn suspends). The `.await`
  gaps between bursts are idle, not charged.
- APs (SMP) run only pool jobs and are otherwise halted, so their cores read
  near-idle unless a parallel job (e.g. `smptest`) is active. That is honest.

All accounting is **TSC-based and sampling-free at the kernel side**: the kernel
maintains monotonic cumulative counters; the **app** takes two snapshots ~1 s
apart and computes the deltas/percentages. This matches how htop reads
`/proc/stat`.

## Architecture — three layers

### Layer 1 — kernel accumulators (cumulative counters)

**1a. Per-process CPU time** — `kernel/src/proc.rs`
`ProcInfo` gains two fields:
```rust
pub cpu_tsc: u64,   // cumulative wasm run-burst cycles
pub mem_bytes: u64, // last-observed wasm linear-memory size in bytes
```
A helper `proc::add_cpu_tsc(pid, delta)` adds to the accumulator;
`proc::set_mem_bytes(pid, bytes)` stores the latest memory size. Both lock the
existing `REGISTRY` mutex and no-op if the pid is gone.

Instrumented in `kernel/src/wasm/fiber.rs` `Fiber::run`: wrap each synchronous
wasmi burst with `rdtsc` reads and charge the delta to `self.pid`:
- around the initial `start.call_resumable(...)`,
- around each `state.resume(...)`.
The `self.dispatch(reason).await` between bursts is the idle gap and is **not**
wrapped. After each burst, also push the current linear-memory size via
`set_mem_bytes` (cheap: `memory.size(&store) * 64 KiB`).

**1b. Per-core CPU busy/idle** — new file `kernel/src/sched/cpustat.rs`
```rust
pub struct CoreStat { pub busy_tsc: AtomicU64, pub idle_tsc: AtomicU64 }
static CORE: [CoreStat; MAX_CPUS] = ...; // MAX_CPUS = 16, matches cpu module
pub fn add_busy(cpu: usize, d: u64);
pub fn add_idle(cpu: usize, d: u64);
pub fn snapshot(out: &mut [(u64,u64)]) -> usize; // returns ncores online
```
- **BSP idle**: `kernel/src/executor/mod.rs` outer loop — `rdtsc` around the
  `enable_and_hlt()` branch; the delta is `add_idle(0, d)`. The poll span
  (between halts) is implicitly busy; we account BSP busy as
  `wall_delta - idle_delta` **in the app** (kernel exposes idle directly and
  busy is derived), OR the loop also brackets `exec.poll()` with rdtsc and calls
  `add_busy(0, d)`. **Chosen: bracket both** (poll → add_busy, hlt → add_idle)
  so the host fn reports both counters uniformly for every core.
- **AP idle/busy**: `kernel/src/cpu/ap.rs` `ap_worker_loop` — `rdtsc` around
  `run_slot` (→ `add_busy(me, d)`) and around `enable_and_hlt` (→ `add_idle(me, d)`).

**1c. TSC frequency** — `kernel/src/boot/clock.rs`
Add `pub fn tsc_per_ms() -> u64` (returns the calibrated `TSC_PER_MS`) and
`pub fn read_tsc() -> u64` (expose the existing private `rdtsc`). `tsc_hz =
tsc_per_ms * 1000`. The app needs `tsc_hz` only to display a sane axis; CPU% is
a ratio of TSC deltas and is frequency-independent, so exact hz is non-critical.

No load-average EMA (dropped as YAGNI; the screen is complete without it).

### Layer 2 — host functions (module `"ruos"`)

Registered in `kernel/src/wasm/host/sysinfo.rs` `link()`.

**`cpustat(buf_ptr, buf_len) -> errno`**
Writes a little-endian blob:
```
[ncores: u32][tsc_per_ms: u64][ per core: busy_tsc: u64, idle_tsc: u64 ]*ncores
```
Returns `0` on success, `ERR_RANGE` if `buf_len` too small. App reads twice and
diffs.

**`proc_stat(buf_ptr, buf_len, used_ptr) -> errno`**
Same framing as the existing `proc_list` but each row is extended:
```
[count: u32]
per row: [pid: u32][start_tick: u64][cpu_tsc: u64][mem_bytes: u64]
         [name_len: u16][pad: u16][name bytes ...]
```
Existing `proc_list` is left untouched so `ps` keeps working; `rtop` uses the
richer `proc_stat`. System memory totals come from the **existing** `meminfo`
host fn (heap + frame counts) — not duplicated here.

### Layer 3 — the app `user/rtop/` (std, `wasm32-wasip1`)

**Rendering: ratatui + a custom ANSI backend.**
crossterm/termion do **not** compile on `wasm32-wasip1` (they need termios
ioctls WASI lacks), so we depend on `ratatui` with `default-features = false`
and implement a small `ratatui::backend::Backend` that emits ANSI escape bytes
to stdout (`fd_write`). Raw mode is entered via the **existing** ruos
`tcgetattr`/`tcsetattr` host fns (the same path the shell uses), not crossterm.

> **De-risk spike (plan Task 0):** before building the UI, confirm a minimal
> `ratatui` + custom-backend binary compiles and links to `wasm32-wasip1`. If it
> does not (dependency pulls a non-wasi crate transitively), fall back to a
> hand-rolled ANSI renderer — the layout and data layers below are
> backend-agnostic. This choice is isolated to the rendering module.

**Backend** (`user/rtop/src/ansi_backend.rs`): implements `draw` (cursor-move +
SGR per dirty cell), `flush`, `size` (fixed 80×24 initially, or a ruos winsize
host fn if available — start fixed), `hide_cursor`/`show_cursor`,
`get/set_cursor_position`, `clear`. Uses the `anes` crate (no_std, MIT/Apache)
to generate sequences. Written once, reusable by future TUI apps.

**Data layer** (`user/rtop/src/sys.rs`): `extern "C"` imports for `cpustat`,
`proc_stat`, `meminfo`, `uptime`, `tcgetattr`, `tcsetattr`; parsers that decode
the LE blobs into Rust structs (`Vec<CoreStat>`, `Vec<Proc>`, `MemInfo`).

**UI layer** (`user/rtop/src/main.rs`):
- Enter alt-screen (`\x1b[?1049h`), hide cursor, enable raw mode.
- Loop: snapshot t0 → sleep 1 s (`poll_oneoff` clock subscription, the WASI path
  the kernel already supports) → snapshot t1 → compute per-core CPU% =
  Δbusy/(Δbusy+Δidle), per-proc CPU% = Δcpu_tsc / Δwall_tsc → render frame.
- Layout (ratatui `Layout`/`Constraint`): header line (uptime, task count, cpu
  count) · per-core `Gauge` bars · system memory `Gauge` (used/total from
  `meminfo`) · process `Table` (PID, CPU%, MEM, TIME+, CMD) sorted CPU% desc.
- Input: non-blocking stdin read; `q` → restore termios, leave alt-screen
  (`\x1b[?1049l`), show cursor, exit 0.

### Wiring
- `Makefile`: append `rtop` to `BIN_TOOLS`.
- `limine.conf`: add `module_path: boot():/bin/rtop.wasm` + `module_cmdline`.
- `user/Cargo.toml`: add the `rtop` workspace member; `user/rtop/Cargo.toml`
  deps `ratatui` (default-features=false), `anes`.

## Data flow

```
timer IRQ 100Hz ─┐
                 ├─► executor outer loop: rdtsc → cpustat::add_busy/idle(0)
Fiber::run ──────┼─► rdtsc around wasmi burst → proc::add_cpu_tsc(pid)
                 │   memory.size → proc::set_mem_bytes(pid)
ap_worker_loop ──┘─► rdtsc → cpustat::add_busy/idle(me)

rtop.wasm: cpustat()/proc_stat()/meminfo()/uptime()
   → snapshot t0 → sleep 1s → snapshot t1 → Δ → % → ratatui render → ANSI → pts
```

## Error handling
- Host fns: bounds-check `buf_len`, return `ERR_RANGE` (existing errno set);
  never write past guest buffer (use `host::mem::guest_write`).
- `add_cpu_tsc`/`set_mem_bytes`/`add_busy`/`add_idle`: silent no-op on bad
  pid/cpu index (counters are best-effort telemetry, never load-bearing).
- App: if `cpustat`/`proc_stat` returns an errno, show an error line and keep
  the last good frame; never crash the terminal in raw mode without restoring it
  (RAII guard that restores termios + alt-screen on drop / on `q` / on panic).
- TSC monotonicity: deltas use `saturating_sub` (guards rare cross-core TSC skew
  under virtualization).

## Testing
1. **Kernel**: assert `ProcInfo.cpu_tsc` is monotonically non-decreasing and
   grows for a CPU-bound wasm proc (a tight-loop test blob), stays ~flat for an
   idle one.
2. **Boot smoke**: spawn `rtop`, confirm it renders ≥1 frame, send `q`, confirm
   clean exit AND that the shell prompt still works afterward (termios restored).
3. **VBox**: run on real VirtualBox (TSC + SMP sensitive — per the SMP findings,
   QEMU-only testing misses VMM-specific TSC/segment issues). Confirm per-core
   bars populate (core 0 active, others idle; run `smptest` in another session
   to light up AP cores).

## Out of scope (YAGNI)
- Load average / run-queue EMA.
- Process tree view, kill-from-UI (kill exists via `proc_kill`; can add a key
  later), scrolling, color themes, configurable refresh.
- Per-core bars for >16 CPUs (MAX_CPUS cap).

## Files touched
- `kernel/src/proc.rs` — ProcInfo fields + add_cpu_tsc/set_mem_bytes
- `kernel/src/wasm/fiber.rs` — rdtsc instrumentation around bursts
- `kernel/src/sched/cpustat.rs` — NEW per-core counters
- `kernel/src/sched/mod.rs` — NEW (or extend) module decl
- `kernel/src/executor/mod.rs` — BSP busy/idle accounting
- `kernel/src/cpu/ap.rs` — AP busy/idle accounting
- `kernel/src/boot/clock.rs` — tsc_per_ms() + read_tsc() getters
- `kernel/src/wasm/host/sysinfo.rs` — cpustat + proc_stat host fns + link
- `user/rtop/{Cargo.toml,src/main.rs,src/ansi_backend.rs,src/sys.rs}` — NEW app
- `user/Cargo.toml` — workspace member
- `Makefile` — BIN_TOOLS += rtop
- `limine.conf` — /bin/rtop.wasm module
- `CHANGELOG/199-26-06-01-rtop.md` — changelog entry
```