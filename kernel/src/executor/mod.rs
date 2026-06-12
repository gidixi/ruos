//! Cooperative async executor for ruos — per-core edition (Step 3b/3c).
//!
//! Built on `embassy-executor`'s low-level `raw::Executor` API because
//! the `x86_64-unknown-none` target isn't covered by any built-in
//! `arch-*` feature. We supply our own `__pender` (which sets a wake
//! flag + cross-core IPI) and our own outer loop (which `hlt`s when no
//! task is ready).
//!
//! Each core owns a slot in `PER_CORE_EXECUTOR` and calls `run_core(cpu)`
//! exactly once, becoming the sole writer and sole poller for that slot.
//! Cross-core task injection (Step 3c) goes through embassy's `SendSpawner`,
//! which enqueues atomically onto the target core's run-queue and calls
//! `__pender(target)` → `wake_core(target)` → targeted IPI.
//!
//! The outer loop uses `sti; hlt` (atomic IRQ-enable + halt) so that
//! the window between checking the wake flag and halting is
//! interrupt-free, eliminating the missed-wake race.

pub mod delay;

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use embassy_executor::raw::Executor as RawExecutor;
use embassy_executor::SendSpawner;
use x86_64::instructions::interrupts;
use crate::kprintln;
use crate::sync::IrqMutex;

// ── C2c: per-request reply for parallel .cwasm exec ──────────────────────────

/// Per-request completion channel for the parallel `.cwasm` exec path (C2c).
/// Created per exec call (one `Arc<ExecReply>` per concurrent app), so multiple
/// apps can run on multiple ComputeApp cores simultaneously without corrupting a
/// shared single slot (the C2b latent bug this fixes).
///
/// The caller (shell fiber via exec_cwasm_parallel) creates an Arc, spawns the
/// AP task with a clone, then awaits `ExecReplyFuture`. The AP task calls
/// `reply.complete(code)` when `run_cwasm` returns, storing the code + waking
/// the waiting shell fiber.
pub struct ExecReply {
    code:  AtomicI32,
    done:  AtomicBool,
    waker: IrqMutex<Option<core::task::Waker>>,
}

impl ExecReply {
    pub fn new() -> alloc::sync::Arc<Self> {
        alloc::sync::Arc::new(Self {
            code:  AtomicI32::new(0),
            done:  AtomicBool::new(false),
            waker: IrqMutex::new(None),
        })
    }

    /// Called by the AP task when run_cwasm returns. Stores the exit code, marks
    /// done, and wakes the awaiting shell fiber (cross-core wake via __pender).
    pub fn complete(&self, code: i32) {
        self.code.store(code, Ordering::SeqCst);
        self.done.store(true, Ordering::SeqCst); // release of code
        if let Some(w) = self.waker.lock().take() { w.wake(); }
    }
}

/// Future that waits for an AP task to complete, returning the exit code.
/// Holds a clone of the `Arc<ExecReply>` so it can be polled from any async
/// context (the shell fiber). While pending, the BSP executor keeps polling
/// all other tasks (net/usb/ssh) — THE GOAL.
pub struct ExecReplyFuture(pub alloc::sync::Arc<ExecReply>);

impl core::future::Future for ExecReplyFuture {
    type Output = i32;
    fn poll(self: core::pin::Pin<&mut Self>, cx: &mut core::task::Context<'_>)
        -> core::task::Poll<i32>
    {
        if self.0.done.load(Ordering::SeqCst) {
            return core::task::Poll::Ready(self.0.code.load(Ordering::SeqCst));
        }
        // Register waker before the second check to avoid a missed-wake race:
        // if the AP completes between the first load and here, it calls wake()
        // on this waker and done becomes true.
        *self.0.waker.lock() = Some(cx.waker().clone());
        if self.0.done.load(Ordering::SeqCst) {
            core::task::Poll::Ready(self.0.code.load(Ordering::SeqCst))
        } else {
            core::task::Poll::Pending
        }
    }
}

/// Round-robin cursor for `pick_compute_core`. Each call advances by 1 and wraps
/// over the online ComputeApp cores, distributing loads across them.
static COMPUTE_CORE_CURSOR: AtomicUsize = AtomicUsize::new(0);

/// Pick a ComputeApp core for the next `.cwasm` exec, using round-robin over all
/// online ComputeApp cores. Returns `None` on 1- or 2-core systems where no
/// ComputeApp AP exists (inline fallback applies).
///
/// Layout: core 0 = BspIo, core 1 = GuiCompositor, core 2+ = ComputeApp.
/// Total cores = 1 (BSP) + cpus_online() (APs).
pub fn pick_compute_core() -> Option<u32> {
    let total = 1 + crate::cpu::cpus_online();
    // Collect ComputeApp cores into a small array to avoid alloc.
    let mut cores = [0u32; crate::cpu::MAX_CPUS];
    let mut n = 0usize;
    for c in 1..total {
        if crate::cpu::core_role(c) == crate::cpu::CoreRole::ComputeApp {
            cores[n] = c;
            n += 1;
        }
    }
    if n == 0 { return None; }
    // Atomically advance cursor and pick the core at cursor % n.
    let idx = COMPUTE_CORE_CURSOR.fetch_add(1, Ordering::Relaxed) % n;
    Some(cores[idx])
}

/// C2c: runs `run_cwasm` on whatever ComputeApp core it is spawned onto. Takes
/// ownership of the bytes, argv, and the per-request `Arc<ExecReply>`. When done
/// it calls `reply.complete(code)` which wakes the awaiting shell fiber via the
/// cross-core pender → IPI chain (Step 2 mechanism).
///
/// `pool_size=4`: up to 4 concurrent `.cwasm` apps can run on ComputeApp cores
/// simultaneously. Each runs on its own core's poll stack so there is no
/// per-core stack contention. The embassy arena holds 4 task states; bump if
/// more concurrent apps are needed (arena is 65536 bytes).
#[embassy_executor::task(pool_size = 4)]
pub async fn run_app_on_core(
    bytes: alloc::boxed::Box<[u8]>,
    argv:  alloc::vec::Vec<alloc::vec::Vec<u8>>,
    pts:   usize,
    reply: alloc::sync::Arc<ExecReply>,
) {
    let cpu  = crate::cpu::cpu_id();
    let code = crate::wasm::wt::run_cwasm(&bytes, argv, Some(pts));
    crate::binfo!("exec-ap", "ran_on=core{} code={}", cpu, code);
    reply.complete(code);
}

/// Parallel `.wasm` (wasmi) exec on a ComputeApp core — the wasmi sibling of
/// [`run_app_on_core`]. Builds the `Fiber` ON the target core (wasmi `Store`/
/// `Instance` are `!Send`, so only the Send `bytes`/`argv`/`cwd`/`name` cross the
/// core boundary, never the fiber). Mirrors the `.wasm` branch of
/// `exec_worker_task` exactly — including the interactive PTY setup (cooked
/// termios + foreground pid) so an interactive tool (rtop) receives keys and
/// `^C` works — then signals the awaiting shell fiber via `reply.complete`.
///
/// Without this, every `.wasm` ran on the single global `exec_worker_task`: a
/// long-running interactive tool parked that worker for its whole lifetime and
/// every other terminal's command queued behind it on EXEC_QUEUE and never ran.
#[embassy_executor::task(pool_size = 4)]
pub async fn run_wasmi_on_core(
    bytes:    alloc::boxed::Box<[u8]>,
    argv:     alloc::vec::Vec<alloc::vec::Vec<u8>>,
    cwd:      alloc::string::String,
    term_pts: usize,
    name:     alloc::string::String,
    reply:    alloc::sync::Arc<ExecReply>,
) {
    let cpu = crate::cpu::cpu_id();
    let code: i32 = match crate::wasm::fiber::Fiber::new(&bytes) {
        Err(e) => {
            kprintln!("ruos: run_wasmi_on_core: instantiate {} failed: {}", name, e);
            126 // cannot execute
        }
        Ok(mut child) => {
            child.set_args(argv);
            child.set_cwd(cwd);
            // Bind the child's stdio to the caller's PTY so output reaches the
            // right terminal (e.g. /dev/pts/N) instead of the boot pts/0.
            child.rebind_stdio_pty(term_pts);
            let pid = crate::proc::register(name);
            child.set_pid(pid);
            // Interactive setup (mirrors exec_worker_task): give the child a
            // cooked terminal + mark it foreground so VINTR (^C) targets it.
            // Apps wanting raw (rtop, nano) flip it themselves and restore on
            // exit; we restore the caller's termios + clear foreground after.
            let saved_termios = crate::pty::termios_snapshot(term_pts);
            crate::pty::force_cooked(term_pts);
            crate::pty::set_foreground(term_pts, Some(pid));
            let code = child.run().await;
            crate::pty::set_foreground(term_pts, None);
            crate::pty::set_termios(term_pts, saved_termios);
            crate::proc::unregister(pid);
            code
        }
    };
    crate::binfo!("exec-ap", "wasmi ran_on=core{} code={}", cpu, code);
    reply.complete(code);
}

/// Per-core wake flag. Index = owner core id. Set by `__pender`, cleared by that
/// core's run loop before each poll. (Was a single global AtomicBool — single-core.)
static WAKE_PENDING: [AtomicBool; crate::cpu::MAX_CPUS] = {
    const F: AtomicBool = AtomicBool::new(true);
    [F; crate::cpu::MAX_CPUS]
};

/// Wrapper that allows `RawExecutor` to live in a `static` array.
///
/// `RawExecutor` is `!Sync` because it carries a `PhantomData<*mut ()>`
/// (the context pointer). The per-core executor model is safe here because
/// each core touches ONLY its own `PER_CORE_EXECUTOR[cpu_id]` slot — it is
/// the sole writer (in `run_core`) and the sole caller of `exec.poll()`.
/// No core ever polls or spawns into another core's executor; cross-core
/// task injection (Step 3c) goes through a per-core spawn queue + IPI,
/// never a direct touch of a remote `RawExecutor`.
struct ExecCell(UnsafeCell<MaybeUninit<RawExecutor>>);
// SAFETY: see the doc comment above — single-writer per slot.
unsafe impl Sync for ExecCell {}

static PER_CORE_EXECUTOR: [ExecCell; crate::cpu::MAX_CPUS] = {
    const E: ExecCell = ExecCell(UnsafeCell::new(MaybeUninit::uninit()));
    [E; crate::cpu::MAX_CPUS]
};

/// Per-core `SendSpawner` slots. Published by `run_core` once that core's executor
/// is initialised. Any core can call `spawn_on(target, token)` to enqueue a task
/// onto `target`'s run-queue via embassy's atomic intrusive list — fully cross-core
/// safe. Embassy then calls `__pender(target)` → `wake_core(target)` → targeted
/// VEC_WAKE IPI → target leaves `hlt` and polls the new task.
///
/// `None` until the target core has entered `run_core`. `spawn_on` returns `Err`
/// in that window (caller may retry). The `IrqMutex` protects against the rare
/// case where a BSP publish and a simultaneous ISR-driven read race on the same
/// slot (in practice the BSP publishes before APs are woken, so the window is tiny).
static PER_CORE_SPAWNER: [IrqMutex<Option<SendSpawner>>; crate::cpu::MAX_CPUS] = {
    const S: IrqMutex<Option<SendSpawner>> = IrqMutex::new(None);
    [S; crate::cpu::MAX_CPUS]
};

/// Spawn `token` onto core `cpu`'s executor from any core. Returns `Err` if
/// `cpu` hasn't entered `run_core` yet (spawner not yet published) or if the
/// task pool for that task is exhausted.
///
/// This is the Step-3c cross-core spawn primitive: embassy enqueues the task
/// atomically on `cpu`'s run-queue and calls `__pender(cpu)` → `wake_core(cpu)`
/// → targeted VEC_WAKE IPI → that core leaves `hlt` and polls it.
pub fn spawn_on<S: Send>(cpu: u32, token: embassy_executor::SpawnToken<S>)
    -> Result<(), embassy_executor::SpawnError>
{
    let g = PER_CORE_SPAWNER[cpu as usize].lock();
    match g.as_ref() {
        Some(s) => s.spawn(token),
        None    => Err(embassy_executor::SpawnError::Busy), // not ready; caller may retry
    }
}

/// Boot-check heartbeat counter for AP1's per-core executor (Step 3b gate).
/// Incremented by `heartbeat_task` every ~20 ms; checked in the interrupts phase
/// boot-check to prove AP1's executor + Delay + timer fire end-to-end.
#[cfg(feature = "boot-checks")]
pub static HEARTBEAT: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// Step 3c boot-check: records which core the cross-spawn probe task RAN on.
/// BSP sets this to u32::MAX; after spawn_on(1, cross_spawn_probe()), the probe
/// (running on core 1) stores cpu_id(). `ran_on==1` proves the full chain:
/// BSP→spawn_on(1)→embassy enqueue on core 1's run-queue→__pender(1)→wake_core(1)
/// →IPI→core 1 leaves hlt→polls the probe.
#[cfg(feature = "boot-checks")]
pub static SPAWN_RAN_ON: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(u32::MAX);

/// Run this core's cooperative executor forever. `cpu` is the dense core id; it
/// is encoded into the executor context so `__pender` (Step 2) wakes THIS core.
///
/// The BSP (cpu 0) spawns the full I/O task set. APs spawn nothing by default
/// here (Step 3c injects tasks later via a queue+IPI); under `boot-checks` AP1
/// also spawns a heartbeat task to prove the per-core executor + Delay + timer
/// chain end-to-end.
pub fn run_core(cpu: u32) -> ! {
    // SAFETY: called exactly once per core, on that core. This core is the sole
    // writer of its `PER_CORE_EXECUTOR[cpu]` slot and the sole caller of poll().
    let exec: &'static RawExecutor = unsafe {
        let slot = &mut *PER_CORE_EXECUTOR[cpu as usize].0.get();
        slot.write(RawExecutor::new(cpu as usize as *mut ())) // context = owner core id
    };

    let spawner = exec.spawner();
    // Publish this core's SendSpawner so other cores can spawn tasks here via
    // `spawn_on`. Must be published BEFORE the first `hlt`, so tasks enqueued
    // from other cores during bringup are never lost. `make_send()` captures
    // the executor's `SyncExecutor` reference (embassy's cross-thread spawn handle).
    *PER_CORE_SPAWNER[cpu as usize].lock() = Some(spawner.make_send());

    if cpu == 0 {
        // BSP owns the I/O task set (unchanged from the old run()).
        crate::binfo!("user", "executor: core 0 spawning tasks");
        spawner.spawn(tick_task()).unwrap();
        // Supervisor 6-detect: BSP polls per-core heartbeats every ~1 s and
        // logs any mute core. Detection only; recovery is 6-recover.
        spawner.spawn(supervisor_task()).unwrap();
        // Net polling is pinned OFF the BSP onto a ComputeApp core (the spawner
        // bootstraps it once that AP is up); ≤2-core falls back to BSP-local.
        spawner.spawn(net_poll_spawner_task()).unwrap();
        spawner.spawn(usb_poll_task()).unwrap();
        spawner.spawn(wifi_poll_task()).unwrap();
        spawner.spawn(console_drain_task()).unwrap();
        // Normal boot: only shell.wasm auto-spawns. init.wasm stays at /init.wasm
        // and server/client.wasm live under /root/ as runnable demo blobs
        // (e.g. `/init.wasm`, `/root/server.wasm`) for debug purposes.
        spawner.spawn(boot_shell_task()).unwrap();
        spawner.spawn(exec_worker_task()).unwrap();
        spawner.spawn(pipeline_worker_task()).unwrap();
        spawner.spawn(ssh_serve_task()).unwrap();
        spawner.spawn(ssh_pty_dispatcher_task()).unwrap();
        spawner.spawn(pty_watchdog_task()).unwrap();
        spawner.spawn(service_dispatcher_task()).unwrap();
        spawner.spawn(unit_scheduler_task()).unwrap();
        spawner.spawn(init_units_task()).unwrap();
        crate::binfo!("user", "executor: core 0 tasks spawned");
    }

    // 3b test hook: the first ComputeApp AP (cpu 2 on SMP≥3, skipped on ≤2
    // where core 1 is the GUI core) runs a heartbeat task to prove the
    // per-core executor + per-core Delay + AP timer chain end-to-end.
    // Core 1 = GuiCompositor runs gui_worker_loop (no executor), so we skip
    // it and use the first ComputeApp AP instead.
    #[cfg(feature = "boot-checks")]
    if cpu == 2 && crate::cpu::core_role(2) == crate::cpu::CoreRole::ComputeApp {
        spawner.spawn(heartbeat_task()).unwrap();
    }

    loop {
        // Supervisor 6-detect: bump once per loop iteration so the BSP supervisor
        // task can tell this core is alive. Even idle cores advance (the LAPIC
        // timer wakes them ~100 Hz → they loop → they bump → they hlt again).
        crate::sched::cpustat::heartbeat_bump(cpu as usize);

        // Clear the wake flag *before* polling so any wakes raised
        // during this poll round are visible to the post-poll check.
        WAKE_PENDING[cpu as usize].store(false, Ordering::SeqCst);
        // SAFETY: raw::Executor::poll must be called serially per core.
        // Each core polls only its own executor — no concurrent access.
        let poll_start = crate::boot::clock::read_tsc();
        unsafe { exec.poll(); }
        // Drain any inter-core messages addressed to this core.
        crate::smp::inbox::drain_inbox(cpu);
        // Drain the compute pool so banded compositing keeps workers.
        // Moved here from ap_worker_loop — any core may take pool jobs.
        while let Some(slot) = crate::smp::pool::take() {
            crate::smp::pool::run_slot(slot, cpu);
        }
        // MT Fase 2: esegui i wasm-thread fiber runnable. Solo core ComputeApp
        // (o il BSP sui sistemi 1-2 core, dove ComputeApp non esiste).
        if crate::wasm::wt::threads::core_allowed(cpu) {
            crate::wasm::wt::threads::expire_timeouts();
            while crate::wasm::wt::threads::run_one(cpu) {}
        }
        crate::sched::cpustat::add_busy(
            cpu as usize, crate::boot::clock::read_tsc().saturating_sub(poll_start));

        // Disable IRQs to atomically check all wake sources and decide
        // between halt and re-poll. Without the disable, an ISR could
        // raise WAKE_PENDING after our load but before our hlt,
        // causing a missed wake.
        interrupts::disable();
        let more = WAKE_PENDING[cpu as usize].load(Ordering::SeqCst)
            || crate::smp::inbox::is_pending(cpu)
            || !crate::smp::pool::is_empty()
            || (crate::wasm::wt::threads::core_allowed(cpu)
                && !crate::wasm::wt::threads::runnable_empty());
        if more {
            interrupts::enable();
            // Re-poll immediately; some waker fired during poll().
        } else {
            // `sti; hlt`: the IRQ that wakes us cannot fire between
            // the two instructions (sti has a 1-instruction shadow).
            // The x86_64 crate exposes this as a safe function.
            let hlt_start = crate::boot::clock::read_tsc();
            interrupts::enable_and_hlt();
            crate::sched::cpustat::add_idle(
                cpu as usize, crate::boot::clock::read_tsc().saturating_sub(hlt_start));
        }
    }
}

/// BSP entry (kept for call-site compatibility). Drives core 0's executor.
pub fn run() -> ! { run_core(0) }

/// 3b boot-check: AP1 heartbeat task. Increments `HEARTBEAT` every ~20 ms
/// (Delay::ticks(2) at 100 Hz) to prove AP1's per-core executor is polling,
/// its Delay future registers in AP1's per-core list, and AP1's LAPIC timer
/// wakes it — the full 3a+3b chain.
#[cfg(feature = "boot-checks")]
#[embassy_executor::task]
async fn heartbeat_task() {
    loop {
        HEARTBEAT.fetch_add(1, Ordering::SeqCst);
        delay::Delay::ticks(2).await; // ~20 ms at 100 Hz; uses THIS core's Delay
    }
}

/// Step 3c boot-check: cross-core spawn probe. The BSP calls
/// `spawn_on(1, cross_spawn_probe())` — if it really runs on core 1,
/// `SPAWN_RAN_ON` will read 1 after the first poll. Send-safe (no non-Send
/// captures), so `SendSpawner::spawn` accepts it.
#[cfg(feature = "boot-checks")]
#[embassy_executor::task]
pub async fn cross_spawn_probe() {
    SPAWN_RAN_ON.store(crate::cpu::cpu_id(), core::sync::atomic::Ordering::SeqCst);
}

// ── C2a: WASI-path (run_cwasm) on AP probe ───────────────────────────────────

/// C2a boot-check: which core the cwasm AP probe task ran on.
/// Starts at u32::MAX (unset). The probe stores `cpu_id()` after run_echo_demo()
/// returns, so the BSP can confirm the full WASI path executed on core 2.
#[cfg(feature = "boot-checks")]
pub static CWASM_AP_RAN_ON: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(u32::MAX);

/// C2a boot-check: exit code from `run_echo_demo()` on the AP.
/// i32::MIN = unset (initial). Stores the actual exit code after the demo runs.
#[cfg(feature = "boot-checks")]
pub static CWASM_AP_CODE: core::sync::atomic::AtomicI32 =
    core::sync::atomic::AtomicI32::new(i32::MIN);

/// C2a boot-check: probe task that runs `run_echo_demo()` (the REAL WASI app
/// path: shared engine + WASI Linker + argv + per-instance Store) on whichever
/// core embassy schedules it on. Spawned onto core 2 (a ComputeApp core) by the
/// boot-check in `interrupts.rs`. Proves the full WASI run_cwasm path works off
/// the BSP — de-risking C2b (routing real exec'd apps to AP cores).
///
/// Send-safe: `run_echo_demo` captures nothing non-Send (uses only `'static`
/// slices and the shared ENGINE spin::Once).
#[cfg(feature = "boot-checks")]
#[embassy_executor::task]
pub async fn cwasm_ap_probe() {
    // run_echo_demo() = run_cwasm(ECHO_CWASM, argv, None) — the REAL WASI app path.
    let code = crate::wasm::wt::run_echo_demo();
    CWASM_AP_RAN_ON.store(crate::cpu::cpu_id(), core::sync::atomic::Ordering::SeqCst);
    CWASM_AP_CODE.store(code, core::sync::atomic::Ordering::SeqCst);
}

// ── C2c: parallel-exec probe (boot-check) ────────────────────────────────────

/// C2c boot-check: which core each parallel probe ran on.
/// Index 0 = first probe (spawned on core 2), index 1 = second (spawned on core 3).
/// u32::MAX = unset. Each probe writes `cpu_id()` here after its loop.
#[cfg(feature = "boot-checks")]
pub static PARALLEL_RAN: [core::sync::atomic::AtomicU32; 2] = [
    core::sync::atomic::AtomicU32::new(u32::MAX),
    core::sync::atomic::AtomicU32::new(u32::MAX),
];

/// C2c boot-check: counter bumped by each probe when it finishes all iterations.
/// BSP waits until this reaches 2 (both probes done). `AtomicU32` used as a
/// simple "done" flag — 0 = neither done, 1 = first done, 2 = both done.
#[cfg(feature = "boot-checks")]
pub static PARALLEL_DONE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// C2c boot-check: accumulator for the parallel probe to prevent loop elision.
/// Each probe stores its final accumulator value here; the BSP reads it after
/// both probes finish to prove neither loop was dead-code-eliminated.
#[cfg(feature = "boot-checks")]
pub static PARALLEL_ACC: [core::sync::atomic::AtomicU64; 2] = [
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
];

/// C2d parallelism probe: runs the CPU-heavy spin `.cwasm` via run_cwasm
/// `iters` times on whatever core embassy scheduled it on. Two of these on
/// cores 2 and 3 concurrently => if wall ≈ single-run, wasm ran in PARALLEL
/// (custom-sync-primitives + per-core TLS working). This is THE real proof.
#[cfg(feature = "boot-checks")]
#[embassy_executor::task(pool_size = 2)]
pub async fn parallel_probe(idx: u32, iters: u32) {
    let mut last: i32 = 0;
    for _ in 0..iters {
        last = crate::wasm::wt::run_spin_demo();
    }
    PARALLEL_ACC[idx as usize].store(last as u64, core::sync::atomic::Ordering::SeqCst);
    PARALLEL_RAN[idx as usize].store(
        crate::cpu::cpu_id(),
        core::sync::atomic::Ordering::SeqCst,
    );
    PARALLEL_DONE.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
}

// ── Step 4 (pty-core): owner-routed write probe (boot-check) ─────────────────

/// Step 4 boot-check: which core the pty-route probe ran on (u32::MAX = unset).
#[cfg(feature = "boot-checks")]
pub static PTY_ROUTE_RAN_ON: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(u32::MAX);

/// Step 4 boot-check: 2 = unset, 1 = the owner ran `pty_write_op` for our routed
/// write (PTY_ROUTED advanced) AND the byte count came back right, 0 = it didn't.
#[cfg(feature = "boot-checks")]
pub static PTY_ROUTE_OK: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(2);

/// Step 4 boot-check: spawned on a ComputeApp core (core 2), this probe calls
/// `route_write_to_owner` for a test pair — an OFF-OWNER write that must hop to
/// the owner (BSP, core 0) over the inbox bus and be processed there. Proves the
/// app-core stdout path routes to the owner instead of locking the pair.
///
/// Send-safe: captures nothing non-Send (only `'static` and the byte literal).
#[cfg(feature = "boot-checks")]
#[embassy_executor::task]
pub async fn pty_route_probe() {
    // Pair 3: not used by the console/SSH path at boot-check time.
    const TEST_IDX: usize = 3;
    let before = crate::pty::PTY_ROUTED.load(core::sync::atomic::Ordering::SeqCst);
    let n = crate::pty::route_write_to_owner(TEST_IDX, b"PTYROUTE").await;
    let after = crate::pty::PTY_ROUTED.load(core::sync::atomic::Ordering::SeqCst);
    let ran_on = crate::cpu::cpu_id();
    // OK iff: the owner ran the op (counter advanced) and accepted all 8 bytes.
    let ok = (after > before) && (n == 8);
    PTY_ROUTE_RAN_ON.store(ran_on, core::sync::atomic::Ordering::SeqCst);
    PTY_ROUTE_OK.store(ok as u32, core::sync::atomic::Ordering::SeqCst);
}

// ── C1: WASM-on-AP probe ──────────────────────────────────────────────────────

/// C1 boot-check: which core the WASM AP probe task ran on.
/// Starts at u32::MAX (unset). The probe stores `cpu_id()` here so the BSP
/// can confirm the task ran on the expected ComputeApp core, not the BSP.
#[cfg(feature = "boot-checks")]
pub static WASM_AP_RAN_ON: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(u32::MAX);

/// C1 boot-check: result of `run_hello_demo()` on the AP.
/// 2 = unset (initial), 1 = ok (demo returned true), 0 = fail.
#[cfg(feature = "boot-checks")]
pub static WASM_AP_OK: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(2);

/// C1 boot-check: probe task that runs `run_hello_demo()` (wasmtime AOT,
/// hello.cwasm) on whichever core embassy schedules it on. Spawned onto core 2
/// (a ComputeApp core) by the boot-check in `interrupts.rs`. Proves the
/// wasmtime AOT runtime instantiates + executes correctly off the BSP —
/// de-risking C2 before we wire real exec'd apps to AP cores.
///
/// Send-safe: `run_hello_demo` captures nothing non-Send (it uses only
/// `'static` slices and the shared ENGINE spin::Once).
#[cfg(feature = "boot-checks")]
#[embassy_executor::task]
pub async fn wasm_ap_probe() {
    let ok = crate::wasm::wt::run_hello_demo();
    WASM_AP_RAN_ON.store(crate::cpu::cpu_id(), core::sync::atomic::Ordering::SeqCst);
    WASM_AP_OK.store(ok as u32, core::sync::atomic::Ordering::SeqCst);
}

/// Runs child WASM processes on behalf of shell fibers that issue exec()
/// calls. This task has its own embassy-allocated stack frame, so wasmi
/// compilation (which is stack-heavy) doesn't overflow the shell fiber.
///
/// C2c: this worker now handles ONLY two cases:
///   1. `compositor.cwasm` — handed off to the GUI core (Step 5 logic unchanged).
///   2. `.wasm` (wasmi) — run inline on the BSP exec-worker stack.
///
/// Non-compositor `.cwasm` apps are now handled at the FIBER level by
/// `exec_cwasm_parallel` (in fiber.rs) which bypasses EXEC_QUEUE entirely,
/// creating per-request `Arc<ExecReply>` + spawning on a ComputeApp core via
/// `pick_compute_core()`. This fixes the latent C2b single-slot corruption bug
/// (two shells exec'ing .cwasm concurrently would race on the old APP_REPLY).
#[embassy_executor::task]
async fn exec_worker_task() {
    use crate::wasm::exec_queue::{EXEC_QUEUE, WaitForRequest};
    use core::sync::atomic::Ordering;
    let _ = crate::proc::register_kernel("exec-worker");
    loop {
        // Wait for a request from a shell fiber.
        let slot = WaitForRequest::new(&EXEC_QUEUE).await;

        // Compositor hand-off: the compositor.cwasm path reaches here through the
        // EXEC_QUEUE (fiber.rs routes compositor.cwasm via EXEC_QUEUE, not the
        // parallel path, because the compositor is a special singleton that owns
        // the GUI core — not a regular parallel app).
        if slot.path.ends_with("compositor.cwasm") {
            let code: i32 = match crate::wasm::read_all(&slot.path).await {
                Err(_) => {
                    kprintln!("ruos: exec_worker: read {} failed", slot.path);
                    127
                }
                Ok(bytes) => {
                    // Step 5: compositor hand-off to the dedicated GUI core.
                    // The compositor owns the CPU and never returns, but we want
                    // the BSP executor to stay alive (for net/usb/ssh). So:
                    //   1. Leak the bytes so they live forever ('static).
                    //   2. Try to hand them to the GUI core (cpu 1 on SMP ≥2).
                    //   3a. GUI core exists → hand-off: complete the EXEC_QUEUE
                    //       handshake (result+done+waker) so the boot shell that
                    //       ran `compositor` gets a return code and keeps going.
                    //       The BSP executor keeps polling I/O. THE GOAL.
                    //   3b. No GUI core (1 CPU) → run inline (today's fallback).
                    let leaked: &'static [u8] =
                        alloc::boxed::Box::leak(bytes.into_boxed_slice());
                    if crate::wasm::wt::wm::send_compositor_to_gui_core(leaked) {
                        // Handed off: the GUI core will run the gate.
                        // Complete the exec handshake so the calling shell
                        // fiber (in boot_shell_task) gets a pid=0 return
                        // code and doesn't hang waiting for `done`.
                        EXEC_QUEUE.result.store(0, Ordering::SeqCst);
                        EXEC_QUEUE.done.store(true, Ordering::SeqCst);
                        if let Some(w) = EXEC_QUEUE.shell_waker.lock().take() {
                            w.wake();
                        }
                        continue; // back to the top of the exec-worker loop
                    }
                    // 1-core fallback: no GUI core → run gate inline (today's
                    // behaviour; the BSP is blocked for the GUI's lifetime).
                    crate::wasm::wt::wm::run_compositor_gate(leaked);
                    0
                }
            };
            EXEC_QUEUE.result.store(code, Ordering::SeqCst);
            EXEC_QUEUE.done.store(true, Ordering::SeqCst);
            if let Some(w) = EXEC_QUEUE.shell_waker.lock().take() {
                w.wake();
            }
            continue;
        }

        // Load and run the child wasm (wasmi path — .wasm files only).
        let code: i32 = match crate::wasm::read_all(&slot.path).await {
            Err(_) => {
                kprintln!("ruos: exec_worker: read {} failed", slot.path);
                127 // command not found
            }
            Ok(bytes) => {
                match crate::wasm::fiber::Fiber::new(&bytes) {
                    Err(e) => {
                        kprintln!("ruos: exec_worker: instantiate {} failed: {}", slot.path, e);
                        126 // cannot execute
                    }
                    Ok(mut child) => {
                        child.set_args(slot.argv);
                        child.set_cwd(slot.cwd);
                        // Bind the child's stdio to the caller's PTY (e.g.
                        // /dev/pts/1 for an SSH-spawned shell) so command
                        // output reaches the SSH channel instead of the
                        // boot framebuffer's /dev/pts/0 default.
                        child.rebind_stdio_pty(slot.term_pts);
                        let pid = crate::proc::register(
                            alloc::string::String::from(
                                slot.path.trim_start_matches('/')
                            )
                        );
                        child.set_pid(pid);
                        // Give the child a sane cooked terminal (so `^C` works
                        // even though the shell runs its line editor in raw
                        // mode) and mark it foreground so VINTR knows which pid
                        // to kill. Restore the caller's termios + clear the
                        // foreground when it exits. Apps wanting raw (rtop,
                        // nano) switch it themselves and their own guard
                        // restores cooked before we restore the shell's mode.
                        let saved_termios = crate::pty::termios_snapshot(slot.term_pts);
                        crate::pty::force_cooked(slot.term_pts);
                        crate::pty::set_foreground(slot.term_pts, Some(pid));
                        let code = child.run().await;
                        crate::pty::set_foreground(slot.term_pts, None);
                        crate::pty::set_termios(slot.term_pts, saved_termios);
                        crate::proc::unregister(pid);
                        code
                    }
                }
            }
        };

        // Signal completion to the waiting shell fiber.
        EXEC_QUEUE.result.store(code, Ordering::SeqCst);
        EXEC_QUEUE.done.store(true, Ordering::SeqCst);
        if let Some(w) = EXEC_QUEUE.shell_waker.lock().take() {
            w.wake();
        }
    }
}

#[embassy_executor::task]
async fn pipeline_worker_task() {
    let _ = crate::proc::register_kernel("pipe-worker");
    crate::wasm::pipeline::worker().await;
}

#[embassy_executor::task]
async fn ssh_serve_task() {
    let _ = crate::proc::register_kernel("sshd");
    crate::ssh::server::serve_loop_pub().await;
}

/// The actual 10 ms network poll loop. Plain async fn so it can be either
/// spawned as a task (`net_poll_task`) on a ComputeApp core or awaited inline
/// on the BSP (the ≤2-core fallback in `net_poll_spawner_task`).
async fn net_poll_loop() {
    loop {
        crate::net::poll();
        delay::Delay::ticks(1).await; // 10 ms @ 100 Hz
    }
}

#[embassy_executor::task]
async fn net_poll_task() {
    net_poll_loop().await;
}

/// Bootstrap (runs on the BSP) that pins network polling OFF the BSP onto the
/// first ComputeApp core, freeing the BSP I/O hub from the 100 Hz `net::poll()`
/// under sustained traffic.
///
/// Safe to move: `net_poll_task` is `Send` (no wasmi state), `NET` is a
/// `spin::Mutex` already accessed cross-core (socket recv/send run from wasm
/// fibers on compute cores today), and the NIC drivers are pure-polling — there
/// is no RX IRQ to co-locate with the poll. So an AP can drive it lock-safely.
///
/// It only bootstraps: retries `spawn_on` until the target AP has published its
/// executor, then exits. On ≤2-core systems (no ComputeApp core) it falls back
/// to polling inline on the BSP (the old behaviour).
#[embassy_executor::task]
async fn net_poll_spawner_task() {
    match crate::cpu::first_compute_app_core() {
        Some(core) => loop {
            match spawn_on(core, net_poll_task()) {
                Ok(()) => {
                    crate::binfo!("net", "net_poll pinned to core{}", core);
                    return;
                }
                // AP's executor not published yet — retry in 10 ms.
                Err(_) => delay::Delay::ticks(1).await,
            }
        },
        // ≤2-core: no ComputeApp core — poll on the BSP (fallback).
        None => net_poll_loop().await,
    }
}

#[embassy_executor::task]
async fn usb_poll_task() {
    loop {
        crate::usb::poll();
        delay::Delay::ticks(1).await; // 10 ms @ 100 Hz
    }
}

/// SP-WIFI-5: drive the encrypted WiFi datapath. Once a WPA2 connect has armed
/// the datapath, pump the radio (drain TX queue, fill RX queue) holding ONLY the
/// CTRLS lock — never NET. No-op until attached, so it's cheap pre-connect.
#[embassy_executor::task]
async fn wifi_poll_task() {
    loop {
        if crate::usb::wifi::datapath::is_attached() {
            crate::usb::wifi::poll_io();
        }
        delay::Delay::ticks(2).await; // ~20 ms @ 100 Hz
    }
}


#[embassy_executor::task(pool_size = 4)]
#[allow(dead_code)] // generic runner; kept for demo blobs / future auto-spawns
async fn wasm_task(path: &'static str) {
    crate::wasm::run_at(path).await;
}

/// The local console (framebuffer + serial, on pts/0) shell, with respawn.
///
/// First launch replays `/etc/init.sh` (the boot sequence). If the user
/// types `exit` — or the shell ever dies — it is respawned with a fresh
/// interactive prompt (no init replay) so the local console is never left
/// dead. Mirrors `init`/getty respawn on Unix: you can't really "log out"
/// of the only physical console, it just comes back.
///
/// SSH sessions are independent (ssh_pty_dispatcher_task on pts/1..NUM_PAIRS)
/// and unaffected: exiting an SSH session never touches this task.
#[embassy_executor::task]
async fn boot_shell_task() {
    let _ = crate::proc::register_kernel("init");
    let mut first = true;
    loop {
        let code = crate::wasm::run_boot_shell(first).await;
        first = false;
        crate::binfo!("user", "boot shell exited code={} — respawning", code);
        // Guard against a tight crash-loop if shell.wasm can't start.
        delay::Delay::ticks(20).await; // 200 ms @ 100 Hz
    }
}

/// Run ONE `shell.wasm` on PTY pair `idx` to completion, then release the pair.
/// Pooled so several PTY shells run CONCURRENTLY — an SSH session and a GUI
/// terminal window (and the next SSH client) each get their own instance. This
/// is what makes SSH and the UI terminal coexist: previously the single
/// dispatcher `await`ed each shell to exit before starting the next, so only one
/// PTY shell could be alive at a time. `pool_size` must be ≥ usable pairs
/// (NUM_PAIRS-1 = 7, pair 0 is the console's own task); kept at NUM_PAIRS=8.
/// (Embassy needs a literal here — keep in sync with `pty::NUM_PAIRS`.)
#[embassy_executor::task(pool_size = 8)]
async fn pty_shell_task(idx: usize, path: alloc::string::String) {
    let bytes = match crate::wasm::read_all(&path).await {
        Ok(b)  => b,
        Err(_) => {
            kprintln!("pty shell spawn: read {} failed", path);
            crate::pty::release(idx); // free the pair so the bridge/window closes
            return;
        }
    };
    let mut fb = match crate::wasm::fiber::Fiber::new(&bytes) {
        Ok(f)  => f,
        Err(e) => {
            kprintln!("pty shell spawn: instantiate {}: {}", path, e);
            crate::pty::release(idx);
            return;
        }
    };
    fb.rebind_stdio_pty(idx);
    // argv = ["shell", "--no-init"] so the shell skips replaying /etc/init.sh +
    // the boot banner (it's not the boot console).
    fb.set_args(alloc::vec![
        b"shell".to_vec(),
        b"--no-init".to_vec(),
    ]);
    let pid = crate::proc::register(
        alloc::string::String::from(path.trim_start_matches('/')),
    );
    fb.set_pid(pid);
    let code = fb.run().await;
    crate::proc::unregister(pid);
    crate::binfo!("pty", "shell on pty {} exited code={}", idx, code);
    crate::pty::release(idx);
}

/// PTY shell dispatcher: drains PTY_QUEUE entries posted by the SSH server (a
/// client opened an interactive shell channel) and by the GUI terminal window
/// (`term.open`). Each entry SPAWNS its own `pty_shell_task` and loops on —
/// shells run concurrently, so SSH and the UI terminal no longer block each other.
#[embassy_executor::task]
async fn ssh_pty_dispatcher_task() {
    let _ = crate::proc::register_kernel("pty-dispatch");
    let spawner = embassy_executor::Spawner::for_current_executor().await;
    loop {
        let next = PTY_QUEUE.lock().pop_front();
        if let Some((idx, path)) = next {
            if spawner.spawn(pty_shell_task(idx, path)).is_err() {
                crate::bwarn!("pty", "shell task pool full; dropping pair {}", idx);
                crate::pty::release(idx);
            }
        } else {
            delay::Delay::ticks(2).await;
        }
    }
}

/// Service dispatcher: drains `SERVICE_QUEUE` entries posted by
/// `crate::service::start`. For each request, loads the named wasm
/// module, builds a Fiber, registers it with `crate::proc`, drives it
/// to completion, then updates the registry status.
///
/// Mirrors `exec_worker_task` (own task stack so wasmi compilation
/// doesn't overflow whatever fiber happened to issue the start), but
/// queues by service name rather than by absolute path.
#[embassy_executor::task]
async fn service_dispatcher_task() {
    use crate::service::{WaitForServiceRequest, UnitReq, mark_running, mark_exited, mark_failed, path_of};
    let _ = crate::proc::register_kernel("svc-dispatch");
    loop {
        let name = match WaitForServiceRequest.await {
            UnitReq::Start(n)   => n,
            UnitReq::Persist(n) => { crate::service::persist(&n).await; continue; }
            UnitReq::Reload     => { crate::service::reload().await;    continue; }
        };
        // Daemon o unit con restart policy → runner supervisionato (pool);
        // oneshot senza policy → exec inline qui sotto.
        let (kind, policy, path) = match crate::service::exec_info_of(&name) {
            Some(t) => t,
            None => {
                crate::bwarn!("svc", "dispatcher: unknown name '{}'", name);
                continue;
            }
        };
        let supervised = matches!(kind, crate::service::UnitKind::Daemon)
            || !matches!(policy, crate::service::RestartPolicy::No);
        if supervised {
            if spawn_on(0, unit_runner_task(name.clone())).is_err() {
                crate::bwarn!("svc", "start {}: no free daemon slot (max {})",
                    name, crate::service::MAX_DAEMONS);
                mark_failed(&name, "noslot");
            }
            continue;
        }
        let bytes = match crate::wasm::read_all(&path).await {
            Ok(b) => b,
            Err(_) => {
                kprintln!("svc: read {} failed", path);
                mark_failed(&name, "read");
                continue;
            }
        };
        let mut fb = match crate::wasm::fiber::Fiber::new(&bytes) {
            Ok(f) => f,
            Err(e) => {
                kprintln!("svc: instantiate {}: {}", path, e);
                mark_failed(&name, "instantiate");
                continue;
            }
        };
        fb.set_args(alloc::vec![name.as_bytes().to_vec()]);
        let pid = crate::proc::register(name.clone());
        fb.set_pid(pid);
        mark_running(&name, pid);
        crate::binfo!("svc", "start name={} pid={} path={}", name, pid, path);
        let code = fb.run().await;
        crate::proc::unregister(pid);
        mark_exited(&name, code);
        crate::binfo!("svc", "exit name={} code={}", name, code);
    }
}

/// Orchestrazione boot delle unit: carica i file, attiva il target Boot,
/// poi PostBoot quando shell/compositor hanno avuto modo di partire.
#[embassy_executor::task]
async fn init_units_task() {
    let _ = crate::proc::register_kernel("init-units");
    crate::service::load_from_disk().await;
    crate::service::activate_target(crate::service::ActivateTarget::Boot).await;
    delay::Delay::ticks(300).await;   // ~3s: shell + compositor su
    crate::service::activate_target(crate::service::ActivateTarget::PostBoot).await;
    crate::binfo!("svc", "unit activation complete");
}

/// Scheduler dei timer unit: polling 1s (robusto a drift/cambi RTC), "fire
/// if due, recompute to future" → niente doppio scatto, niente backfill.
#[embassy_executor::task]
async fn unit_scheduler_task() {
    let _ = crate::proc::register_kernel("unit-sched");
    loop {
        delay::Delay::ticks(100).await; // ~1s @100Hz
        let ticks = crate::timer::ticks();
        let epoch = crate::rtc::to_unix_epoch(&crate::rtc::now());
        for (idx, sched, next_fire) in crate::service::timers_due_snapshot() {
            let due = match sched {
                crate::service::schedule::Schedule::EveryTicks(_)
                | crate::service::schedule::Schedule::BootPlus(_) => ticks >= next_fire,
                _ => epoch >= next_fire,
            };
            if !due { continue; }
            if let Some(unit) = crate::service::timer_fired(idx, epoch, ticks) {
                crate::binfo!("svc", "timer fired -> start {}", unit);
                if let Err(e) = crate::service::start(&unit) {
                    crate::bwarn!("svc", "timer start {}: {}", unit, e);
                }
            }
        }
    }
}

/// Runner di un'unità supervisionata: esegue il child, applica la restart
/// policy con backoff esponenziale, esce su stop_requested o policy esaurita.
/// I daemon non hanno PTY: stdout → console (pts 0). `.cwasm` gira su un
/// compute core via `exec_cwasm_inner`; `.wasm` inline (wasmi) sul BSP.
#[embassy_executor::task(pool_size = 8)] // = service::MAX_DAEMONS (l'attr vuole un letterale)
async fn unit_runner_task(name: alloc::string::String) {
    use crate::service::{self, RestartPolicy};
    loop {
        let Some((_kind, policy, path)) = service::exec_info_of(&name) else { return; };
        let bytes = match crate::wasm::read_all(&path).await {
            Ok(b) => b,
            Err(_) => { service::mark_failed(&name, "read"); return; }
        };
        let pid = crate::proc::register(name.clone());
        service::mark_running(&name, pid);
        let started = crate::timer::ticks();
        crate::binfo!("svc", "runner start name={} pid={} path={}", name, pid, path);

        let code = if path.ends_with(".cwasm") {
            crate::wasm::fiber::exec_cwasm_inner(
                bytes, alloc::vec![name.as_bytes().to_vec()], 0).await
        } else {
            match crate::wasm::fiber::Fiber::new(&bytes) {
                Ok(mut fb) => {
                    fb.set_args(alloc::vec![name.as_bytes().to_vec()]);
                    fb.set_pid(pid);
                    fb.run().await
                }
                Err(_) => {
                    service::mark_failed(&name, "instantiate");
                    crate::proc::unregister(pid);
                    return;
                }
            }
        };
        crate::proc::unregister(pid);
        crate::binfo!("svc", "runner exit name={} code={}", name, code);

        // Uptime > 60s → crash transitorio recuperato: reset del backoff.
        if crate::timer::ticks().saturating_sub(started) > 6_000 {
            service::reset_restarts(&name);
        }
        if service::take_stop_requested(&name) {
            service::mark_exited(&name, code);
            return;
        }
        let restart = matches!(policy, RestartPolicy::Always)
            || (matches!(policy, RestartPolicy::OnFailure) && code != 0);
        if !restart {
            service::mark_exited(&name, code);
            return;
        }
        service::mark_restarting(&name);
        let n = service::bump_restarts(&name);
        let wait = crate::service::schedule::backoff_ticks(n.saturating_sub(1));
        crate::binfo!("svc", "restart name={} #{} in {} ticks", name, n, wait);
        delay::Delay::ticks(wait).await;
    }
}

static PTY_QUEUE:
    spin::Mutex<alloc::collections::VecDeque<(usize, alloc::string::String)>> =
    spin::Mutex::new(alloc::collections::VecDeque::new());

/// Enqueue a shell-on-PTY spawn request. Picked up by ssh_pty_dispatcher_task.
pub fn enqueue_shell_pty(idx: usize, path: alloc::string::String) {
    PTY_QUEUE.lock().push_back((idx, path));
}

/// Drains PTY 0 master output and writes each byte to the framebuffer console.
/// Shell output path: shell.wasm fd_write → PtySlaveFile::write → ldisc::process_output
/// → pair[0].master_out → this task → CONSOLE.
#[embassy_executor::task]
async fn console_drain_task() {
    let _ = crate::proc::register_kernel("console-drain");
    loop {
        let b = crate::pty::master_output_read(0).await;
        x86_64::instructions::interrupts::without_interrupts(|| {
            use core::fmt::Write;
            let mut c = crate::console::CONSOLE.lock();
            let buf = [b];
            let s = core::str::from_utf8(&buf).unwrap_or("?");
            let _ = c.write_str(s);
        });
    }
}

/// Decide se il watchdog deve reap-are un pair: SOLO le sessioni SSH leak-ate.
/// I terminali GUI locali (`LocalGui`) non vanno mai uccisi per idle — dormono
/// nel compositor e restano vivi finché la finestra esiste. Pura → ovvia.
fn should_reap(origin: crate::pty::PtyOrigin, idle_exceeded: bool) -> bool {
    origin == crate::pty::PtyOrigin::Ssh && idle_exceeded
}

/// Software watchdog over SSH-spawned PTY pairs. Boot shell on pair 0 is
/// excluded — it has no notion of "session end" and a local user is allowed
/// to leave the prompt idle indefinitely.
///
/// For pairs 1..NUM_PAIRS, if a claimed pair has had no I/O activity for
/// IDLE_LIMIT_TICKS, we mark it for shutdown. The slave reader (the wasm
/// shell blocked on stdin) wakes with EOF, exits its loop, the dispatcher
/// releases the pair, and the slot is free for the next SSH connection.
///
/// Without this, an SSH session that drops abruptly and somehow bypasses
/// the bridge's request_shutdown (kernel bug, sunset internal failure)
/// would leak the pair until reboot. The watchdog is the safety net under
/// the per-session SIGHUP.
#[embassy_executor::task]
async fn pty_watchdog_task() {
    let _ = crate::proc::register_kernel("watchdog");
    const CHECK_INTERVAL_TICKS: u64 = 1000;  // 10 s @ 100 Hz
    const IDLE_LIMIT_TICKS:     u64 = 30000; // 5 min @ 100 Hz — backstop for
                                             // LEAKED pairs only; a live bridge
                                             // heartbeats touch_activity so a
                                             // connected idle session is never
                                             // reaped (see ssh/sunset_io.rs).
    loop {
        delay::Delay::ticks(CHECK_INTERVAL_TICKS).await;
        let now = crate::timer::ticks();
        for idx in 1..crate::pty::NUM_PAIRS {
            if !crate::pty::is_claimed(idx) { continue; }
            if crate::pty::is_shutdown(idx) { continue; }
            let last = crate::pty::last_activity(idx);
            let idle_exceeded = now.saturating_sub(last) > IDLE_LIMIT_TICKS;
            if should_reap(crate::pty::origin(idx), idle_exceeded) {
                crate::bwarn!(
                    "pty", "watchdog: pair {} idle {}s — shutting down",
                    idx, now.saturating_sub(last) / 100,
                );
                crate::pty::request_shutdown(idx);
            }
        }
    }
}

#[embassy_executor::task]
async fn tick_task() {
    // Boot scheduler heartbeat. Was a debug print loop; now silent —
    // keeps the executor with at least one always-live task slot
    // available so the run queue never becomes empty.
    loop {
        delay::Delay::ticks(1000).await; // 10s heartbeat
    }
}

/// Supervisor 6-detect task (BSP only). Runs on core 0's executor (Step 5 freed
/// it). Every ~1 s it snapshots per-core heartbeat counters, waits, and compares.
/// A core whose counter did not advance is logged as "mute". DETECTION ONLY —
/// recovery (killing stuck WASM instances) requires per-core process registries
/// (6-recover, a later step). Just log liveness; add no cross-core locks.
#[embassy_executor::task]
async fn supervisor_task() {
    let mut prev = [0u64; crate::cpu::MAX_CPUS];
    let mut first = true;
    let mut muted = false;
    loop {
        // Total cores = BSP (always 1) + online APs. cpus_online() counts APs only.
        let n = (1 + crate::cpu::cpus_online()) as usize;
        delay::Delay::ticks(100).await; // ~1 s at 100 Hz
        let mut alive = 0u32;
        let mut mute  = 0u32;
        for c in 0..n {
            let h = crate::sched::cpustat::heartbeat(c);
            if h != prev[c] {
                alive += 1;
            } else if !first {
                mute += 1;
            }
            prev[c] = h;
        }
        if first {
            crate::binfo!("super", "supervisor up, watching {} cores", n);
            first = false;
        } else if mute > 0 {
            crate::bwarn!("super", "mute cores={} alive={}/{}", mute, alive, n);
            muted = true;
        } else if muted {
            // Recovered: log the transition back to all-alive ONCE, then go
            // quiet. Steady-state health is silent — no per-second INFO spam
            // flooding the serial wire and the dmesg ring buffer.
            crate::binfo!("super", "all {} cores alive (recovered)", n);
            muted = false;
        }
    }
}

/// Wake the owner core: set its WAKE_PENDING and, if we are NOT that core, send it a
/// targeted VEC_WAKE IPI so it leaves `hlt`. Used by `__pender` and any cross-core
/// signaller. ISR-safe: atomic store + (maybe) one IPI write, no locks/allocs.
pub fn wake_core(owner: u32) {
    WAKE_PENDING[owner as usize].store(true, Ordering::SeqCst);
    if crate::cpu::cpu_id() != owner {
        crate::apic::lapic::send_ipi(crate::cpu::lapic_id_of(owner), crate::idt::VEC_WAKE);
    }
}

/// Embassy's "we have work, please poll" callback. Called from
/// `Waker::wake()` — possibly from an ISR. `context` carries the OWNER core id
/// (encoded as a pointer when the executor was created). Wakes that core —
/// including a cross-core IPI if the wake originated on a different core.
///
/// MUST be ISR-safe: no locks, no allocations.
#[no_mangle]
extern "Rust" fn __pender(context: *mut ()) {
    wake_core(context as usize as u32);
}
