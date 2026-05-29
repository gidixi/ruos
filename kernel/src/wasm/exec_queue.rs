//! Deferred WASM exec: the shell fiber queues a child exec here; the
//! `exec_worker_task` in executor picks it up and runs the child in its
//! own async context (separate from the shell's stack), then writes the
//! exit code and signals completion.
//!
//! Design: single-slot (one child at a time). Shell waits on completion
//! before issuing the next exec. Sufficient for a sequential init script.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use core::task::Waker;
use spin::Mutex;

/// A pending exec request.
pub struct ExecSlot {
    pub path: alloc::string::String,
    pub argv: Vec<Vec<u8>>,
    pub exit_code: *mut i32, // pointer into caller's memory — valid until done flag
}

// SAFETY: single-CPU kernel; ExecSlot is only accessed while the shell
// fiber is suspended (waiting for done), so the raw pointer is stable.
unsafe impl Send for ExecSlot {}
unsafe impl Sync for ExecSlot {}

pub struct ExecQueue {
    /// Pending request, if any.
    pub pending: Mutex<Option<ExecSlot>>,
    /// Set true by exec_worker_task when the child exits.
    pub done: AtomicBool,
    /// Exit code written by exec_worker_task.
    pub result: AtomicI32,
    /// Waker for the shell fiber waiting on completion.
    pub shell_waker: Mutex<Option<Waker>>,
    /// Waker for the exec_worker_task waiting for a new request.
    pub worker_waker: Mutex<Option<Waker>>,
}

pub static EXEC_QUEUE: ExecQueue = ExecQueue {
    pending: Mutex::new(None),
    done: AtomicBool::new(false),
    result: AtomicI32::new(0),
    shell_waker: Mutex::new(None),
    worker_waker: Mutex::new(None),
};

impl ExecQueue {
    /// Called from shell fiber's dispatch(Exec{...}): posts a request and
    /// returns a future that resolves to the exit code.
    pub fn post_and_wait(
        &'static self,
        path: alloc::string::String,
        argv: Vec<Vec<u8>>,
    ) -> ExecFuture {
        ExecFuture::new(self, path, argv)
    }
}

/// Future returned to the shell fiber. Resolves once exec_worker_task
/// finishes the child and writes the result.
pub struct ExecFuture {
    queue: &'static ExecQueue,
    posted: bool,
    path: alloc::string::String,
    argv: Vec<Vec<u8>>,
}

impl ExecFuture {
    fn new(
        queue: &'static ExecQueue,
        path: alloc::string::String,
        argv: Vec<Vec<u8>>,
    ) -> Self {
        Self { queue, posted: false, path, argv }
    }
}

impl core::future::Future for ExecFuture {
    type Output = i32;

    fn poll(
        mut self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<i32> {
        if !self.posted {
            // Clear done flag before posting.
            self.queue.done.store(false, Ordering::SeqCst);
            self.queue.result.store(0, Ordering::SeqCst);

            // Post the request.
            let path = core::mem::take(&mut self.path);
            let argv = core::mem::take(&mut self.argv);
            *self.queue.pending.lock() = Some(ExecSlot {
                path,
                argv,
                exit_code: core::ptr::null_mut(), // unused; result via AtomicI32
            });
            self.posted = true;

            // Wake the worker.
            if let Some(w) = self.queue.worker_waker.lock().take() {
                w.wake();
            }
        }

        if self.queue.done.load(Ordering::SeqCst) {
            return core::task::Poll::Ready(self.queue.result.load(Ordering::SeqCst));
        }

        // Store our waker so the worker can wake us when done.
        *self.queue.shell_waker.lock() = Some(cx.waker().clone());
        core::task::Poll::Pending
    }
}

/// Future for the worker task: resolves when a new request is posted.
pub struct WaitForRequest {
    queue: &'static ExecQueue,
}

impl WaitForRequest {
    pub fn new(queue: &'static ExecQueue) -> Self { Self { queue } }
}

impl core::future::Future for WaitForRequest {
    type Output = ExecSlot;

    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<ExecSlot> {
        if let Some(slot) = self.queue.pending.lock().take() {
            return core::task::Poll::Ready(slot);
        }
        *self.queue.worker_waker.lock() = Some(cx.waker().clone());
        core::task::Poll::Pending
    }
}
