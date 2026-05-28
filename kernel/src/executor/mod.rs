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
    spawner.spawn(bootstrap_task()).unwrap();

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

/// Minimal task that proves the executor links and runs. Prints the
/// expected boot sentinel, then parks forever so the executor never
/// runs out of tasks. Later tasks (Task 2, Task 3) replace this with
/// real work.
#[embassy_executor::task]
async fn bootstrap_task() {
    kprintln!("ruos: executor up");
    core::future::pending::<()>().await;
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
