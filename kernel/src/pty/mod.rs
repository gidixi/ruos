//! PTY subsystem. Static pool of 4 pseudo-terminal pairs.

pub mod termios;
pub mod ldisc;
pub mod pair;
pub mod spsc;

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

/// Owner core for pair `idx`. v1: the BSP owns every pair (all PTY input + SSH
/// master-read run on the BSP). Extension point: round-robin to dedicated
/// pty-cores when core count grows — the SPSC ring stays valid as long as ONE
/// core (the owner) is the sole producer of `idx`'s slave input.
pub fn pty_owner(_idx: usize) -> u32 { 0 }

/// Per-pair slave-input ring (replaces `PtyPair::slave_rx`). Producer = owner
/// (line discipline); consumer = the app core reading stdin.
static SLAVE_RX: [spsc::SpscRing; NUM_PAIRS] = [
    spsc::SpscRing::new(), spsc::SpscRing::new(),
    spsc::SpscRing::new(), spsc::SpscRing::new(),
];
pub fn slave_rx_ring(idx: usize) -> &'static spsc::SpscRing { &SLAVE_RX[idx] }

/// Foreground pid per pair as an atomic (0 = none) so the app-side read path
/// can check kill/EOF without taking the owner-local pair lock.
static FOREGROUND: [core::sync::atomic::AtomicU32; NUM_PAIRS] = [
    core::sync::atomic::AtomicU32::new(0), core::sync::atomic::AtomicU32::new(0),
    core::sync::atomic::AtomicU32::new(0), core::sync::atomic::AtomicU32::new(0),
];

/// Foreground app pid for pair `idx`, or `None` (at the shell prompt). Read by
/// the app-side stdin path to detect a `^C`/kill and report EOF.
pub fn foreground_pid(idx: usize) -> Option<u32> {
    if idx >= NUM_PAIRS { return None; }
    match FOREGROUND[idx].load(Ordering::SeqCst) {
        0 => None,
        pid => Some(pid),
    }
}

/// Per-pair slave-input consumer waker. The consumer (app core) registers its
/// `Waker` here; the producer (owner) wakes it after `push`. Lives outside the
/// pair lock so producer + consumer never share the pair lock just for the
/// waker handoff.
static SLAVE_WAKER: [crate::sync::IrqMutex<Option<core::task::Waker>>; NUM_PAIRS] =
    [crate::sync::IrqMutex::new(None), crate::sync::IrqMutex::new(None),
     crate::sync::IrqMutex::new(None), crate::sync::IrqMutex::new(None)];

/// Consumer (app core): register the waker to be notified when input arrives.
pub fn register_slave_waker(idx: usize, w: core::task::Waker) {
    if idx >= NUM_PAIRS { return; }
    *SLAVE_WAKER[idx].lock() = Some(w);
}

/// Producer (owner): wake the slave consumer after pushing input (or on shutdown).
pub fn wake_slave(idx: usize) {
    if idx >= NUM_PAIRS { return; }
    if let Some(w) = SLAVE_WAKER[idx].lock().take() { w.wake(); }
}

/// Off-owner caller: send `buf` to pair `idx`'s owner; the owner appends it to
/// master_out via process_output. Returns bytes accepted. One bus msg per call
/// (same granularity as the owner-local lock path — no per-byte regression).
pub async fn route_write_to_owner(idx: usize, buf: &[u8]) -> usize {
    let owner = pty_owner(idx);
    let mut input = alloc::vec::Vec::with_capacity(4 + buf.len());
    input.extend_from_slice(&(idx as u32).to_le_bytes());
    input.extend_from_slice(buf);
    let n = crate::smp::inbox::request(owner, pty_write_op, input.into_boxed_slice()).await;
    n as usize
}

/// Owner-side bus op: input = [idx:u32 le][bytes...]; run process_output locally
/// into the owner's `master_out`. Plain `fn` (no captures) → valid bus op ptr.
/// Takes ONLY the pair lock (no registry / other lock) → no cross-core
/// contention (the owner is the sole locker) and no new ordering hazard.
fn pty_write_op(input: &[u8]) -> u64 {
    if input.len() < 4 { return 0; }
    let idx = u32::from_le_bytes([input[0], input[1], input[2], input[3]]) as usize;
    if idx >= NUM_PAIRS { return 0; }
    let bytes = &input[4..];
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut g = PAIRS[idx].lock();
        for &b in bytes { ldisc::process_output(&mut g, b); }
    });
    #[cfg(feature = "boot-checks")]
    PTY_ROUTED.fetch_add(1, Ordering::SeqCst);
    bytes.len() as u64
}

/// Boot-check counter: incremented by the owner each time it runs `pty_write_op`
/// (i.e. an off-owner write was routed to it). Used by the pty-route gate.
#[cfg(feature = "boot-checks")]
pub static PTY_ROUTED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

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
    let kill = ldisc::process_input(idx, &mut g, byte);
    drop(g);
    // Wake the slave consumer (app core) after dropping the pair lock: input was
    // pushed into the SPSC ring (or a `^C` requires the reader to re-poll and see
    // the kill). The ring's Release + this wake order the cross-core handoff.
    wake_slave(idx);
    // `^C` on a foreground app: request its cooperative kill AFTER releasing the
    // pair lock (request_kill takes the proc REGISTRY lock; keeping the orders
    // disjoint avoids any pair<->registry deadlock).
    if let Some(pid) = kill {
        crate::proc::request_kill(pid);
    }
    touch_activity(idx);
}

/// Set (or clear) the foreground app pid for pair `idx`. The exec worker calls
/// this when it starts a child on a terminal and again (with `None`) when the
/// child exits, so `^C` knows which process to interrupt.
pub fn set_foreground(idx: usize, pid: Option<u32>) {
    if idx >= NUM_PAIRS { return; }
    FOREGROUND[idx].store(pid.unwrap_or(0), Ordering::SeqCst);
}

/// Snapshot pair `idx`'s termios so the exec worker can restore it after a
/// foreground child (which may have switched the terminal to raw) exits.
pub fn termios_snapshot(idx: usize) -> termios::Termios {
    use x86_64::instructions::interrupts::without_interrupts;
    if idx >= NUM_PAIRS { return termios::Termios::default_cooked(); }
    without_interrupts(|| PAIRS[idx].lock().termios)
}

/// Overwrite pair `idx`'s termios (used to restore a snapshot).
pub fn set_termios(idx: usize, t: termios::Termios) {
    use x86_64::instructions::interrupts::without_interrupts;
    if idx >= NUM_PAIRS { return; }
    without_interrupts(|| { PAIRS[idx].lock().termios = t; });
}

/// Reset pair `idx` to a sane cooked terminal. The exec worker calls this
/// before running a foreground child so the child gets canonical input + `^C`
/// signal handling regardless of the shell's raw line-editing mode. Apps that
/// want raw (rtop, nano) set it themselves via `tcsetattr`.
pub fn force_cooked(idx: usize) {
    set_termios(idx, termios::Termios::default_cooked());
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
    let claimed = CLAIMED[idx]
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok();
    if claimed {
        // Start the idle clock NOW. Without this the fresh session inherits the
        // PREVIOUS session's release timestamp (or 0 on first use); once uptime
        // exceeds the idle limit the watchdog would shut the new session down
        // within one check interval — i.e. seconds after connecting.
        LAST_ACTIVITY[idx].store(crate::timer::ticks(), Ordering::Relaxed);
    }
    claimed
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
    // Wake the slave consumer so a blocked reader unblocks immediately and sees
    // EOF. The waker lives outside the pair lock (cross-core slot).
    wake_slave(idx);
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

/// Read one byte from pair `idx`'s slave (stdin), waiting up to `timeout_ticks`
/// (100 Hz). Returns the byte (`0..=255`), `-1` on timeout, or `-2` on EOF
/// (the pair was shut down). Unlike `vfs::read`, this operates directly on the
/// pair and ALWAYS resolves — no fd-table entry is taken, so an early timeout
/// never strands the fd. Lets an interactive TUI (rtop) refresh on a clock
/// while still reacting instantly to keystrokes.
pub async fn slave_read_one_timeout(idx: usize, timeout_ticks: u64) -> i32 {
    if idx >= NUM_PAIRS { return -2; }
    use core::future::Future;
    let mut delay = core::pin::pin!(crate::executor::delay::Delay::ticks(timeout_ticks));
    core::future::poll_fn(|cx| {
        use core::task::Poll;
        // Lock-free read from the SPSC ring (no pair lock). Register the
        // consumer waker BEFORE the final emptiness re-check so a byte the
        // producer pushes right after we look still wakes us.
        if let Some(b) = slave_rx_ring(idx).pop() {
            touch_activity(idx);
            return Poll::Ready(b as i32);
        }
        if SHUTDOWN[idx].load(Ordering::Relaxed)
            || foreground_pid(idx).map(|p| crate::proc::is_kill_pending(p)).unwrap_or(false)
        {
            // Pair hung up, or the foreground app was `^C`'d / killed — report
            // EOF so the reader unwinds and the fiber's kill check fires.
            return Poll::Ready(-2);
        }
        // Register the waker, then re-check the ring (avoid a lost wake from a
        // push that lands between the pop above and the registration).
        register_slave_waker(idx, cx.waker().clone());
        if let Some(b) = slave_rx_ring(idx).pop() {
            touch_activity(idx);
            return Poll::Ready(b as i32);
        }
        if SHUTDOWN[idx].load(Ordering::Relaxed)
            || foreground_pid(idx).map(|p| crate::proc::is_kill_pending(p)).unwrap_or(false)
        {
            return Poll::Ready(-2);
        }
        // No byte yet — arm the timeout. `Delay` registers its own waker; the
        // future always completes, so it is dropped (freeing its slot) cleanly.
        if delay.as_mut().poll(cx).is_ready() {
            return Poll::Ready(-1);
        }
        Poll::Pending
    }).await
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
