//! PTY subsystem. Static pool of 4 pseudo-terminal pairs.

pub mod termios;
pub mod ldisc;
pub mod pair;

use spin::Mutex;
use pair::PtyPair;

pub const NUM_PAIRS: usize = 4;

static PAIRS: [Mutex<PtyPair>; NUM_PAIRS] = [
    Mutex::new(PtyPair::new()),
    Mutex::new(PtyPair::new()),
    Mutex::new(PtyPair::new()),
    Mutex::new(PtyPair::new()),
];

pub fn pair(idx: usize) -> &'static Mutex<PtyPair> {
    &PAIRS[idx]
}

/// Called from boot fs phase after vfs::init.
pub fn init() {
    crate::binfo!("pty", "{} pairs ready", NUM_PAIRS);
}

/// Push a byte into pair `idx`'s master input. Runs line discipline.
/// Safe to call from ISR context (uses without_interrupts not needed
/// since ISR already has IF=0; just lock).
pub fn master_input_push(idx: usize, byte: u8) {
    if idx >= NUM_PAIRS { return; }
    let mut g = PAIRS[idx].lock();
    ldisc::process_input(&mut g, byte);
    drop(g);
    touch_activity(idx);
}

/// Non-blocking poll of pair `idx`'s master output. Returns `Some(byte)` if
/// available, `None` otherwise. Used by the SSH session bridge.
pub fn master_output_try(idx: usize) -> Option<u8> {
    if idx >= NUM_PAIRS { return None; }
    use x86_64::instructions::interrupts::without_interrupts;
    let b = without_interrupts(|| {
        let mut g = PAIRS[idx].lock();
        g.master_out.pop_front()
    });
    if b.is_some() { touch_activity(idx); }
    b
}

/// Number of bytes currently queued in pair `idx`'s master output, without
/// consuming them. The SSH bridge uses this to know when a finished shell's
/// output has been fully drained before closing the channel.
pub fn master_output_len(idx: usize) -> usize {
    if idx >= NUM_PAIRS { return 0; }
    use x86_64::instructions::interrupts::without_interrupts;
    without_interrupts(|| PAIRS[idx].lock().master_out.len())
}

/// Atomic claim of pair `idx`. Returns true on success; subsequent claims
/// of the same pair return false until [`release`] is called.
pub fn try_claim(idx: usize) -> bool {
    if idx >= NUM_PAIRS { return false; }
    use core::sync::atomic::Ordering;
    CLAIMED[idx].compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok()
}

pub fn release(idx: usize) {
    if idx >= NUM_PAIRS { return; }
    use core::sync::atomic::Ordering;
    CLAIMED[idx].store(false, Ordering::SeqCst);
    // Reset shutdown so the next claim starts clean. Activity timestamp
    // resets too — a freshly claimed pty shouldn't inherit the previous
    // session's idle counter.
    SHUTDOWN[idx].store(false, Ordering::SeqCst);
    LAST_ACTIVITY[idx].store(crate::timer::ticks(), Ordering::Relaxed);
}

/// `true` while pair `idx` is claimed by a running process. The SSH bridge
/// polls this: once the spawned shell exits and the dispatcher releases the
/// pair, this flips to `false`, signalling the bridge to close the channel.
pub fn is_claimed(idx: usize) -> bool {
    if idx >= NUM_PAIRS { return false; }
    use core::sync::atomic::Ordering;
    CLAIMED[idx].load(Ordering::SeqCst)
}

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
static CLAIMED: [AtomicBool; NUM_PAIRS] = [
    AtomicBool::new(false), AtomicBool::new(false),
    AtomicBool::new(false), AtomicBool::new(false),
];

/// Per-pair shutdown flag. Set by [`request_shutdown`] (e.g. SSH session
/// dropped, watchdog idle timeout). When set, `PtySlaveFile::read` returns
/// `Ok(0)` (EOF) once `slave_rx` is drained, so a shell blocked on stdin
/// reads EOF and exits cleanly. Cleared on `release` for the next claim.
static SHUTDOWN: [AtomicBool; NUM_PAIRS] = [
    AtomicBool::new(false), AtomicBool::new(false),
    AtomicBool::new(false), AtomicBool::new(false),
];

/// Per-pair last-activity tick (100 Hz). Updated on any input/output the
/// pair sees so the watchdog can detect genuinely idle pairs vs busy ones.
static LAST_ACTIVITY: [AtomicU64; NUM_PAIRS] = [
    AtomicU64::new(0), AtomicU64::new(0),
    AtomicU64::new(0), AtomicU64::new(0),
];

/// Mark `idx` for shutdown and wake any task blocked reading the slave.
/// Idempotent. Once a pair is in shutdown, `PtySlaveFile::read` returns
/// `Ok(0)` after draining anything still buffered in `slave_rx`.
pub fn request_shutdown(idx: usize) {
    if idx >= NUM_PAIRS { return; }
    SHUTDOWN[idx].store(true, Ordering::SeqCst);
    // Take + wake the slave waker so a blocked reader unblocks immediately.
    let waker = x86_64::instructions::interrupts::without_interrupts(|| {
        PAIRS[idx].lock().slave_waker.take()
    });
    if let Some(w) = waker { w.wake(); }
}

pub fn is_shutdown(idx: usize) -> bool {
    if idx >= NUM_PAIRS { return false; }
    SHUTDOWN[idx].load(Ordering::Relaxed)
}

pub fn touch_activity(idx: usize) {
    if idx >= NUM_PAIRS { return; }
    LAST_ACTIVITY[idx].store(crate::timer::ticks(), Ordering::Relaxed);
}

pub fn last_activity(idx: usize) -> u64 {
    if idx >= NUM_PAIRS { return 0; }
    LAST_ACTIVITY[idx].load(Ordering::Relaxed)
}

/// Future-friendly read of one byte from pair `idx`'s master output.
/// Used by console_drain_task.
pub async fn master_output_read(idx: usize) -> u8 {
    use core::future::poll_fn;
    use core::task::Poll;
    poll_fn(|cx| {
        use x86_64::instructions::interrupts::without_interrupts;
        without_interrupts(|| {
            let mut g = PAIRS[idx].lock();
            match g.master_out.pop_front() {
                Some(b) => Poll::Ready(b),
                None => {
                    g.master_waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        })
    }).await
}
