//! Inter-core message bus: per-core inbox + targeted IPI + async request/reply.
//! A sender on core A posts a message to core B's inbox and IPIs B; B drains its
//! inbox in its run loop, runs the message op, publishes the result, and wakes the
//! sender's Waker. No shared mutable state crosses the boundary — only the owned
//! message (Box) and the reply slot (Arc). The Box is allocated on A and dropped on
//! B (cross-core free) — the magazine allocator (Step 1b) handles that.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use core::task::{Context, Poll, Waker};
use core::future::Future;
use core::pin::Pin;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::collections::VecDeque;
use crate::sync::IrqMutex;
use crate::cpu::MAX_CPUS;

/// Result channel for one request. Shared (Arc) between sender and the executing core.
pub struct ReplySlot {
    result: AtomicU64,
    done: AtomicBool,
    waker: IrqMutex<Option<Waker>>,
}

impl ReplySlot {
    fn new() -> Self {
        Self {
            result: AtomicU64::new(0),
            done: AtomicBool::new(false),
            waker: IrqMutex::new(None),
        }
    }

    fn complete(&self, value: u64) {
        self.result.store(value, Ordering::SeqCst);
        self.done.store(true, Ordering::SeqCst);              // Release of the result
        if let Some(w) = self.waker.lock().take() { w.wake(); } // cross-core wake
    }
}

/// A message to run an op on the target core. `op` is a pure fn; the reply is
/// delivered asynchronously via the ReplySlot + the sender's Waker.
pub struct InboxMsg {
    op: fn(&[u8]) -> u64,
    input: Box<[u8]>,
    reply: Arc<ReplySlot>,
}

struct CoreInbox {
    queue: IrqMutex<VecDeque<Box<InboxMsg>>>,
    pending: AtomicBool,
}

impl CoreInbox {
    const fn new() -> Self {
        Self {
            queue: IrqMutex::new(VecDeque::new()),
            pending: AtomicBool::new(false),
        }
    }
}

static PER_CORE_INBOX: [CoreInbox; MAX_CPUS] = {
    const I: CoreInbox = CoreInbox::new();
    [I; MAX_CPUS]
};

/// IPI handler hook: mark `cpu`'s inbox pending (called from inbox_handler in idt.rs).
pub fn mark_pending(cpu: u32) {
    PER_CORE_INBOX[cpu as usize].pending.store(true, Ordering::SeqCst);
}

/// Post `msg` to `target`'s inbox and IPI it. SeqCst publish before the IPI so the
/// target observes the enqueue when it drains (the IPI is the wake, not the fence —
/// but the IrqMutex enqueue + SeqCst pending flag order the handoff).
fn enqueue(target: u32, msg: Box<InboxMsg>) {
    PER_CORE_INBOX[target as usize].queue.lock().push_back(msg);
    PER_CORE_INBOX[target as usize].pending.store(true, Ordering::SeqCst);
    let lapic = crate::cpu::lapic_id_of(target);
    crate::apic::lapic::send_ipi(lapic, crate::idt::VEC_INBOX);
}

/// Async: run `op(input)` on `target` core, await the u64 result.
pub fn request(target: u32, op: fn(&[u8]) -> u64, input: Box<[u8]>) -> ReplyFuture {
    let reply = Arc::new(ReplySlot::new());
    enqueue(target, Box::new(InboxMsg { op, input, reply: reply.clone() }));
    ReplyFuture { reply }
}

/// Future that resolves once the target core completes the message op.
pub struct ReplyFuture { reply: Arc<ReplySlot> }

impl Future for ReplyFuture {
    type Output = u64;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u64> {
        if self.reply.done.load(Ordering::SeqCst) {
            Poll::Ready(self.reply.result.load(Ordering::SeqCst))
        } else {
            // Register/refresh the waker BEFORE re-checking done (avoid lost wake).
            *self.reply.waker.lock() = Some(cx.waker().clone());
            if self.reply.done.load(Ordering::SeqCst) {
                Poll::Ready(self.reply.result.load(Ordering::SeqCst))
            } else {
                Poll::Pending
            }
        }
    }
}

/// Drain THIS core's inbox: run each queued message's op and complete its reply.
/// Called from the core's run loop (BSP poll loop / AP worker loop) when pending.
/// Returns the number drained.
///
/// IMPORTANT: the queue lock is NOT held while running `op` or calling `complete`
/// (which may `wake()` → `__pender` → IPI). The inner block drops the guard first.
pub fn drain_inbox(cpu: u32) -> usize {
    let inbox = &PER_CORE_INBOX[cpu as usize];
    if !inbox.pending.swap(false, Ordering::SeqCst) { return 0; }
    let mut n = 0;
    loop {
        // Pop one message, releasing the lock BEFORE running the op.
        let msg = { inbox.queue.lock().pop_front() };
        match msg {
            Some(m) => {
                let value = (m.op)(&m.input);
                m.reply.complete(value);   // wakes the sender (cross-core)
                n += 1;
            }
            None => break,
        }
    }
    n
}

/// True if this core has inbox work pending (for the halt decision).
pub fn is_pending(cpu: u32) -> bool {
    PER_CORE_INBOX[cpu as usize].pending.load(Ordering::SeqCst)
}
