//! Cooperative async executor for ruos.
//!
//! Built on `embassy-executor`'s low-level `raw::Executor` API because
//! the `x86_64-unknown-none` target isn't covered by any built-in
//! `arch-*` feature. We supply our own `__pender` (which sets a wake
//! flag) and our own outer loop (which `hlt`s when no task is ready).
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

/// Set by `__pender` whenever a waker is signalled. Cleared by the
/// outer loop before each `poll()`. If set after `poll()` returns,
/// the loop re-polls instead of halting.
static WAKE_PENDING: AtomicBool = AtomicBool::new(true);

/// Wrapper that allows `RawExecutor` to live in a `static`.
///
/// `RawExecutor` is `!Sync` because it carries a `PhantomData<*mut ()>`
/// (the context pointer). Our kernel is single-CPU with no concurrent
/// access; the `UnsafeCell<MaybeUninit<…>>` pattern is safe here.
struct ExecCell(UnsafeCell<MaybeUninit<RawExecutor>>);
// SAFETY: single-CPU kernel; no concurrent access is possible.
unsafe impl Sync for ExecCell {}

static EXECUTOR: ExecCell = ExecCell(UnsafeCell::new(MaybeUninit::uninit()));

/// Drive the kernel forever as a cooperative task system.
///
/// Spawns the bootstrap task, then enters the idle loop: poll, check
/// for pending wakes, halt with IRQs enabled, repeat.  Returns never.
pub fn run() -> ! {
    // SAFETY: called exactly once from kmain after init. The
    // UnsafeCell is only written here; the `&'static` reference is
    // valid for the remainder of the kernel's lifetime.
    let exec: &'static RawExecutor = unsafe {
        let slot = &mut *EXECUTOR.0.get();
        slot.write(RawExecutor::new(core::ptr::null_mut()))
    };

    let spawner = exec.spawner();
    kprintln!("ruos: executor: spawning tasks");
    spawner.spawn(tick_task()).unwrap();
    spawner.spawn(net_poll_task()).unwrap();
    spawner.spawn(wasm_task("/init.wasm")).unwrap();
    spawner.spawn(wasm_task("/server.wasm")).unwrap();
    spawner.spawn(wasm_task("/client.wasm")).unwrap();
    spawner.spawn(wasm_task("/bin/shell.wasm")).unwrap();
    spawner.spawn(exec_worker_task()).unwrap();
    kprintln!("ruos: executor: all tasks spawned, entering poll loop");

    loop {
        // Clear the wake flag *before* polling so any wakes raised
        // during this poll round are visible to the post-poll check.
        WAKE_PENDING.store(false, Ordering::SeqCst);
        // SAFETY: raw::Executor::poll must be called serially. The
        // kernel is single-threaded and we call it only from here.
        unsafe { exec.poll(); }

        // Disable IRQs to atomically check WAKE_PENDING and decide
        // between halt and re-poll. Without the disable, an ISR could
        // raise WAKE_PENDING after our load but before our hlt,
        // causing a missed wake.
        interrupts::disable();
        if WAKE_PENDING.load(Ordering::SeqCst) {
            interrupts::enable();
            // Re-poll immediately; some waker fired during poll().
        } else {
            // `sti; hlt`: the IRQ that wakes us cannot fire between
            // the two instructions (sti has a 1-instruction shadow).
            // The x86_64 crate exposes this as a safe function.
            interrupts::enable_and_hlt();
        }
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
                        child.run().await
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
async fn net_poll_task() {
    loop {
        crate::net::poll();
        delay::Delay::ticks(1).await; // 10 ms @ 100 Hz
    }
}

#[embassy_executor::task(pool_size = 4)]
async fn wasm_task(path: &'static str) {
    crate::wasm::run_at(path).await;
}

#[embassy_executor::task]
async fn tick_task() {
    kprintln!("ruos: executor up");
    let mut n: u32 = 0;
    loop {
        delay::Delay::ticks(100).await; // 1s @ 100 Hz
        kprintln!("ruos: async tick={}", n);
        n = n.wrapping_add(1);
    }
}

/// Embassy's "we have work, please poll" callback. Called from
/// `Waker::wake()` — possibly from an ISR. Setting the atomic is the
/// only signal the outer loop needs.
///
/// MUST be ISR-safe: no locks, no allocations.
#[no_mangle]
extern "Rust" fn __pender(_context: *mut ()) {
    WAKE_PENDING.store(true, Ordering::SeqCst);
}
