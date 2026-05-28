//! Single-poll synchronous driver for VFS futures. Tmpfs and device futures
//! never yield Pending, so a noop waker plus a poll loop completes them on
//! the first poll. When `embassy-executor` lands in Step 9, this helper
//! becomes obsolete or stays as a debug-only escape hatch.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

static VTABLE: RawWakerVTable = RawWakerVTable::new(
    |_| RawWaker::new(core::ptr::null(), &VTABLE),
    |_| {},
    |_| {},
    |_| {},
);

pub fn block_on<F: Future>(mut fut: F) -> F::Output {
    let raw = RawWaker::new(core::ptr::null(), &VTABLE);
    // SAFETY: VTABLE has all four fn pointers and the data pointer is unused.
    let waker = unsafe { Waker::from_raw(raw) };
    let mut cx = Context::from_waker(&waker);
    // SAFETY: `fut` is owned here and never moved while pinned.
    let mut fut = unsafe { Pin::new_unchecked(&mut fut) };
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending  => continue,
        }
    }
}
