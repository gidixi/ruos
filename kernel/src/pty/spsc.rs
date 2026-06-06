//! Single-producer / single-consumer lock-free byte ring for PTY slave input.
//! Producer = the pair's owner core (line discipline); consumer = the app core
//! reading stdin. Cross-core handoff via head/tail atomics; the consumer's Waker
//! is woken by the producer (Waker::wake is cross-core-safe via __pender).
use core::sync::atomic::{AtomicUsize, Ordering};
use core::cell::UnsafeCell;

const CAP: usize = 4096;              // power of two; ample for tty input
const MASK: usize = CAP - 1;

pub struct SpscRing {
    buf: UnsafeCell<[u8; CAP]>,
    head: AtomicUsize,                // next write index (producer)
    tail: AtomicUsize,                // next read index (consumer)
}
// SAFETY: exactly one producer core touches `head` + buf[head..]; exactly one
// consumer core touches `tail` + buf[tail..]. The atomics order the handoff.
unsafe impl Sync for SpscRing {}

impl SpscRing {
    pub const fn new() -> Self {
        SpscRing {
            buf: UnsafeCell::new([0; CAP]),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }
    /// Producer: push one byte. Returns false if full (byte dropped).
    pub fn push(&self, b: u8) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) >= CAP { return false; } // full
        // SAFETY: producer-exclusive slot.
        unsafe { (*self.buf.get())[head & MASK] = b; }
        self.head.store(head.wrapping_add(1), Ordering::Release);
        true
    }
    /// Consumer: pop one byte, or None if empty.
    pub fn pop(&self) -> Option<u8> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail == head { return None; } // empty
        // SAFETY: consumer-exclusive slot.
        let b = unsafe { (*self.buf.get())[tail & MASK] };
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(b)
    }
    pub fn is_empty(&self) -> bool {
        self.tail.load(Ordering::Acquire) == self.head.load(Ordering::Acquire)
    }
}
