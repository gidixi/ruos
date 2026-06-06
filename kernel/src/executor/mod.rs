//! Cooperative async executor for ruos — per-core edition (Step 3b).
//!
//! Built on `embassy-executor`'s low-level `raw::Executor` API because
//! the `x86_64-unknown-none` target isn't covered by any built-in
//! `arch-*` feature. We supply our own `__pender` (which sets a wake
//! flag + cross-core IPI) and our own outer loop (which `hlt`s when no
//! task is ready).
//!
//! Each core owns a slot in `PER_CORE_EXECUTOR` and calls `run_core(cpu)`
//! exactly once, becoming the sole writer and sole poller for that slot.
//! Cross-core task injection goes through a per-core spawn queue + IPI
//! (Step 3c) — never a direct touch of a remote `RawExecutor`.
//!
//! The outer loop uses `sti; hlt` (atomic IRQ-enable + halt) so that
//! the window between checking the wake flag and halting is
//! interrupt-free, eliminating the missed-wake race.

pub mod delay;

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, Ordering};
use embassy_executor::raw::Executor as RawExecutor;
use x86_64::instructions::interrupts;
use crate::kprintln;

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

/// Boot-check heartbeat counter for AP1's per-core executor (Step 3b gate).
/// Incremented by `heartbeat_task` every ~20 ms; checked in the interrupts phase
/// boot-check to prove AP1's executor + Delay + timer fire end-to-end.
#[cfg(feature = "boot-checks")]
pub static HEARTBEAT: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

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
    if cpu == 0 {
        // BSP owns the I/O task set (unchanged from the old run()).
        crate::binfo!("user", "executor: core 0 spawning tasks");
        spawner.spawn(tick_task()).unwrap();
        spawner.spawn(net_poll_task()).unwrap();
        spawner.spawn(usb_poll_task()).unwrap();
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
        crate::binfo!("user", "executor: core 0 tasks spawned");
    }

    // 3b test hook: AP 1 runs a heartbeat task to prove the per-core executor
    // + per-core Delay + AP timer work end-to-end. Only under boot-checks.
    #[cfg(feature = "boot-checks")]
    if cpu == 1 {
        spawner.spawn(heartbeat_task()).unwrap();
    }

    loop {
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
        crate::sched::cpustat::add_busy(
            cpu as usize, crate::boot::clock::read_tsc().saturating_sub(poll_start));

        // Disable IRQs to atomically check all wake sources and decide
        // between halt and re-poll. Without the disable, an ISR could
        // raise WAKE_PENDING after our load but before our hlt,
        // causing a missed wake.
        interrupts::disable();
        let more = WAKE_PENDING[cpu as usize].load(Ordering::SeqCst)
            || crate::smp::inbox::is_pending(cpu)
            || !crate::smp::pool::is_empty();
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

/// Runs child WASM processes on behalf of shell fibers that issue exec()
/// calls. This task has its own embassy-allocated stack frame, so wasmi
/// compilation (which is stack-heavy) doesn't overflow the shell fiber.
#[embassy_executor::task]
async fn exec_worker_task() {
    use crate::wasm::exec_queue::{EXEC_QUEUE, WaitForRequest};
    use core::sync::atomic::Ordering;
    loop {
        // Wait for a request from a shell fiber.
        let slot = WaitForRequest::new(&EXEC_QUEUE).await;

        // Router: `.cwasm` → Wasmtime AOT runtime; `.wasm` → wasmi.
        if slot.path.ends_with(".cwasm") {
            let code: i32 = match crate::wasm::read_all(&slot.path).await {
                Err(_) => {
                    kprintln!("ruos: exec_worker: read {} failed", slot.path);
                    127
                }
                Ok(bytes) => {
                    // Compositor GATE: `compositor` resolves to /bin/compositor.cwasm
                    // (the reactor cwasm shipped under that name). It owns the CPU
                    // and never returns — like the single-GUI path — so the exec
                    // task blocks here. That is intentional for the visual gate.
                    if slot.path.ends_with("compositor.cwasm") {
                        crate::wasm::wt::wm::run_compositor_gate(&bytes);
                    }
                    let pid = crate::proc::register(
                        alloc::string::String::from(slot.path.trim_start_matches('/')),
                    );
                    // stdout/stderr bound to the caller's PTY slave (reaches the
                    // terminal / SSH channel, like a rebound wasmi tool). stdin
                    // is EOF for now (blocking PTY reads need epoch/async — TODO).
                    let c = crate::wasm::wt::run_cwasm(&bytes, slot.argv, Some(slot.term_pts));
                    crate::proc::unregister(pid);
                    c
                }
            };
            EXEC_QUEUE.result.store(code, Ordering::SeqCst);
            EXEC_QUEUE.done.store(true, Ordering::SeqCst);
            if let Some(w) = EXEC_QUEUE.shell_waker.lock().take() {
                w.wake();
            }
            continue;
        }

        // Load and run the child wasm.
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
    crate::wasm::pipeline::worker().await;
}

#[embassy_executor::task]
async fn ssh_serve_task() {
    crate::ssh::server::serve_loop_pub().await;
}

#[embassy_executor::task]
async fn net_poll_task() {
    loop {
        crate::net::poll();
        delay::Delay::ticks(1).await; // 10 ms @ 100 Hz
    }
}

#[embassy_executor::task]
async fn usb_poll_task() {
    loop {
        crate::usb::poll();
        delay::Delay::ticks(1).await; // 10 ms @ 100 Hz
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
/// SSH sessions are independent (ssh_pty_dispatcher_task on pts/1..3) and
/// unaffected: exiting an SSH session never touches this task.
#[embassy_executor::task]
async fn boot_shell_task() {
    let mut first = true;
    loop {
        let code = crate::wasm::run_boot_shell(first).await;
        first = false;
        crate::binfo!("user", "boot shell exited code={} — respawning", code);
        // Guard against a tight crash-loop if shell.wasm can't start.
        delay::Delay::ticks(20).await; // 200 ms @ 100 Hz
    }
}

/// SSH PTY dispatcher: drains PTY_QUEUE entries posted by the SSH server
/// when a client opens an interactive shell channel. Each entry spawns
/// `shell.wasm` on the requested PTY pair, binding FDs 0/1/2 to its slave.
#[embassy_executor::task]
async fn ssh_pty_dispatcher_task() {
    loop {
        let next = PTY_QUEUE.lock().pop_front();
        if let Some((idx, path)) = next {
            let bytes = match crate::wasm::read_all(&path).await {
                Ok(b)  => b,
                Err(_) => {
                    kprintln!("ssh shell spawn: read {} failed", path);
                    crate::pty::release(idx); // free the pair so the bridge closes
                    continue;
                }
            };
            let mut fb = match crate::wasm::fiber::Fiber::new(&bytes) {
                Ok(f)  => f,
                Err(e) => {
                    kprintln!("ssh shell spawn: instantiate {}: {}", path, e);
                    crate::pty::release(idx);
                    continue;
                }
            };
            fb.rebind_stdio_pty(idx);
            // argv = ["shell", "--no-init"] so the SSH shell skips replaying
            // /etc/init.sh + the boot banner.
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
            crate::binfo!("ssh", "shell on pty {} exited code={}", idx, code);
            crate::pty::release(idx);
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
    use crate::service::{WaitForServiceRequest, mark_running, mark_exited, mark_failed, path_of};
    loop {
        let name = WaitForServiceRequest.await;
        let path = match path_of(name) {
            Some(p) => p,
            None    => {
                crate::bwarn!("svc", "dispatcher: unknown name '{}'", name);
                continue;
            }
        };
        let bytes = match crate::wasm::read_all(path).await {
            Ok(b) => b,
            Err(_) => {
                kprintln!("svc: read {} failed", path);
                mark_failed(name, "read");
                continue;
            }
        };
        let mut fb = match crate::wasm::fiber::Fiber::new(&bytes) {
            Ok(f) => f,
            Err(e) => {
                kprintln!("svc: instantiate {}: {}", path, e);
                mark_failed(name, "instantiate");
                continue;
            }
        };
        fb.set_args(alloc::vec![name.as_bytes().to_vec()]);
        let pid = crate::proc::register(alloc::string::String::from(name));
        fb.set_pid(pid);
        mark_running(name, pid);
        crate::binfo!("svc", "start name={} pid={} path={}", name, pid, path);
        let code = fb.run().await;
        crate::proc::unregister(pid);
        mark_exited(name, code);
        crate::binfo!("svc", "exit name={} code={}", name, code);
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
            if now.saturating_sub(last) > IDLE_LIMIT_TICKS {
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
