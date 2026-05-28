//! Async-aware keyboard byte queue.
//!
//! The keyboard ISR (`keyboard::handler`) decodes a scancode into an
//! ASCII byte and pushes it via `push_from_isr`. A consumer task awaits
//! `read_char()`, which suspends on a stored `Waker` until the next
//! byte arrives.
//!
//! Concurrency:
//! - ISR side: holds the `STATE` lock briefly to push + take the
//!   waker; calls `wake()` *outside* the lock to avoid re-entering
//!   the executor while holding the queue.
//! - Task side: wraps `STATE.lock()` in `without_interrupts` so the
//!   keyboard IRQ can't preempt it mid-critical-section and deadlock.
//!
//! Overflow policy: drop the byte and bump `DROPPED`. A 64-byte buffer
//! at 100 keystrokes/s gives ~640 ms of slack — plenty for cooperative
//! scheduling.

use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::{AtomicU64, Ordering};
use core::task::{Context, Poll, Waker};
use spin::Mutex;
use x86_64::instructions::interrupts::without_interrupts;

const BUF_LEN: usize = 64;

struct State {
    buf: [u8; BUF_LEN],
    head: usize, // next-to-read
    tail: usize, // next-to-write
    waker: Option<Waker>,
}

impl State {
    const fn new() -> Self {
        Self {
            buf: [0; BUF_LEN],
            head: 0,
            tail: 0,
            waker: None,
        }
    }
    fn is_empty(&self) -> bool {
        self.head == self.tail
    }
    fn is_full(&self) -> bool {
        (self.tail + 1) % BUF_LEN == self.head
    }
}

static STATE: Mutex<State> = Mutex::new(State::new());
pub static DROPPED: AtomicU64 = AtomicU64::new(0);

/// Called by the keyboard ISR for each decoded byte. Non-blocking;
/// drops the byte if the queue is full (incrementing `DROPPED`).
pub fn push_from_isr(b: u8) {
    let waker = {
        let mut s = STATE.lock();
        if s.is_full() {
            DROPPED.fetch_add(1, Ordering::Relaxed);
            None
        } else {
            let tail = s.tail;
            s.buf[tail] = b;
            s.tail = (tail + 1) % BUF_LEN;
            s.waker.take()
        }
    };
    // Wake outside the lock — wake() may eventually re-enter the
    // executor, and we don't want to nest the locks.
    if let Some(w) = waker {
        w.wake();
    }
}

/// Async future that resolves to the next available byte.
pub fn read_char() -> ReadChar {
    ReadChar
}

pub struct ReadChar;

impl Future for ReadChar {
    type Output = u8;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u8> {
        without_interrupts(|| {
            let mut s = STATE.lock();
            if s.is_empty() {
                s.waker = Some(cx.waker().clone());
                Poll::Pending
            } else {
                let b = s.buf[s.head];
                s.head = (s.head + 1) % BUF_LEN;
                Poll::Ready(b)
            }
        })
    }
}
