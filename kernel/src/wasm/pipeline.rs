//! Concurrent pipeline executor: runs N wasm stages joined by pipes.
//!
//! The shell posts a whole pipeline; `pipeline_worker_task` (executor) runs
//! `run_pipeline` on its own task stack. Inside, all stages are polled
//! concurrently via `JoinAll` — polls are SEQUENTIAL within one task, so peak
//! native (wasmi) stack is one stage deep, not the sum. Each stage closes its
//! own pipe-end FDs on exit so EOF flows downstream as producers finish.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use spin::Mutex;

use crate::kprintln;

/// Max stages per pipeline (sanity / resource bound).
pub const PIPE_MAX_STAGES: usize = 4;

type Stage = (String, Vec<Vec<u8>>); // (path, argv)

struct PipeRequest {
    stages: Vec<Stage>,
    cwd: String,
    term_pts: usize,
}

struct PipelineQueue {
    pending: Mutex<Option<PipeRequest>>,
    done: core::sync::atomic::AtomicBool,
    result: core::sync::atomic::AtomicI32,
    shell_waker: Mutex<Option<Waker>>,
    worker_waker: Mutex<Option<Waker>>,
}

pub static PIPELINE: PipelineQueue = PipelineQueue {
    pending: Mutex::new(None),
    done: core::sync::atomic::AtomicBool::new(false),
    result: core::sync::atomic::AtomicI32::new(0),
    shell_waker: Mutex::new(None),
    worker_waker: Mutex::new(None),
};

pub struct PipelineFuture {
    posted: bool,
    stages: Vec<Stage>,
    cwd: String,
    term_pts: usize,
}

pub fn post_and_wait(stages: Vec<Stage>, cwd: String, term_pts: usize) -> PipelineFuture {
    PipelineFuture { posted: false, stages, cwd, term_pts }
}

impl Future for PipelineFuture {
    type Output = i32;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<i32> {
        use core::sync::atomic::Ordering;
        if !self.posted {
            PIPELINE.done.store(false, Ordering::SeqCst);
            PIPELINE.result.store(0, Ordering::SeqCst);
            let stages = core::mem::take(&mut self.stages);
            let cwd = core::mem::take(&mut self.cwd);
            let term_pts = self.term_pts;
            *PIPELINE.pending.lock() = Some(PipeRequest { stages, cwd, term_pts });
            self.posted = true;
            if let Some(w) = PIPELINE.worker_waker.lock().take() { w.wake(); }
        }
        if PIPELINE.done.load(Ordering::SeqCst) {
            return Poll::Ready(PIPELINE.result.load(Ordering::SeqCst));
        }
        *PIPELINE.shell_waker.lock() = Some(cx.waker().clone());
        Poll::Pending
    }
}

struct WaitForPipeline;
impl Future for WaitForPipeline {
    type Output = PipeRequest;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<PipeRequest> {
        if let Some(req) = PIPELINE.pending.lock().take() {
            return Poll::Ready(req);
        }
        *PIPELINE.worker_waker.lock() = Some(cx.waker().clone());
        Poll::Pending
    }
}

pub async fn worker() {
    use core::sync::atomic::Ordering;
    loop {
        let req = WaitForPipeline.await;
        let code = run_pipeline(req.stages, req.cwd, req.term_pts).await;
        PIPELINE.result.store(code, Ordering::SeqCst);
        PIPELINE.done.store(true, Ordering::SeqCst);
        if let Some(w) = PIPELINE.shell_waker.lock().take() { w.wake(); }
    }
}

struct JoinAll {
    futs: Vec<Option<Pin<Box<dyn Future<Output = i32>>>>>,
    codes: Vec<i32>,
}

impl Future for JoinAll {
    type Output = Vec<i32>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Vec<i32>> {
        let me = self.get_mut();
        let mut all_done = true;
        for (i, slot) in me.futs.iter_mut().enumerate() {
            if let Some(f) = slot.as_mut() {
                match f.as_mut().poll(cx) {
                    Poll::Ready(code) => { me.codes[i] = code; *slot = None; }
                    Poll::Pending => { all_done = false; }
                }
            }
        }
        if all_done {
            Poll::Ready(core::mem::take(&mut me.codes))
        } else {
            Poll::Pending
        }
    }
}

pub async fn run_pipeline(stages: Vec<Stage>, cwd: String, term_pts: usize) -> i32 {
    let n = stages.len();
    crate::binfo!("pipe", "run n={} first={}", n,
        stages.first().map(|s| s.0.as_str()).unwrap_or("?"));
    if n == 0 { return 0; }
    if n > PIPE_MAX_STAGES {
        kprintln!("ruos: pipeline too long ({} > {})", n, PIPE_MAX_STAGES);
        return 1;
    }

    let mut pipes: Vec<(crate::vfs::Fd, crate::vfs::Fd)> = Vec::with_capacity(n - 1);
    for _ in 0..n.saturating_sub(1) {
        pipes.push(crate::vfs::pipe());
    }

    let mut futs: Vec<Option<Pin<Box<dyn Future<Output = i32>>>>> = Vec::with_capacity(n);

    for (i, (path, argv)) in stages.into_iter().enumerate() {
        let mut close_fds: Vec<crate::vfs::Fd> = Vec::new();
        let stdin_fd = if i > 0 { Some(pipes[i - 1].0) } else { None };
        let stdout_fd = if i + 1 < n { Some(pipes[i].1) } else { None };
        if let Some(fd) = stdin_fd { close_fds.push(fd); }
        if let Some(fd) = stdout_fd { close_fds.push(fd); }

        let cwd = cwd.clone();
        let fut = async move {
            let bytes = match crate::wasm::read_all(&path).await {
                Ok(b) => b,
                Err(_) => {
                    kprintln!("ruos: pipeline: read {} failed", path);
                    for fd in &close_fds { let _ = crate::vfs::block_on(crate::vfs::close(*fd)); }
                    return 127;
                }
            };
            let mut fb = match crate::wasm::fiber::Fiber::new(&bytes) {
                Ok(f) => f,
                Err(e) => {
                    kprintln!("ruos: pipeline: instantiate {}: {}", path, e);
                    for fd in &close_fds { let _ = crate::vfs::block_on(crate::vfs::close(*fd)); }
                    return 126;
                }
            };
            fb.set_args(argv);
            fb.set_cwd(cwd);
            // Inherit the calling shell's terminal for all 3 FDs, then
            // override the pipe-connected ends. Leaves stderr + the chain
            // endpoints (first stdin, last stdout) on the shell's PTY.
            fb.rebind_stdio_pty(term_pts);
            if let Some(fd) = stdin_fd { fb.bind_fd(0, fd); }
            if let Some(fd) = stdout_fd { fb.bind_fd(1, fd); }
            let pid = crate::proc::register(
                alloc::string::String::from(path.trim_start_matches('/')),
            );
            fb.set_pid(pid);
            crate::binfo!("pipe", "stage {} start in={:?} out={:?}", path, stdin_fd, stdout_fd);
            let code = fb.run().await;
            crate::binfo!("pipe", "stage {} exit code={}", path, code);
            crate::proc::unregister(pid);
            for fd in &close_fds { let _ = crate::vfs::block_on(crate::vfs::close(*fd)); }
            code
        };
        futs.push(Some(Box::pin(fut)));
    }

    let codes = JoinAll { codes: alloc::vec![0i32; n], futs }.await;
    *codes.last().unwrap_or(&0)
}
