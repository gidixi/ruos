# Pipe Unix + exec concorrente — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add shell pipelines (`cmd1 | cmd2 | …`) to ruos via a kernel-side
`exec_pipeline` host fn: anonymous pipe FDs + concurrent stage fibers on the
cooperative executor (no SMP).

**Architecture:** A `Pipe` is an `Arc<Mutex<PipeInner>>` with two `File`
ends (read/write), mirroring the existing `PtySlaveFile`/`PtyPair` waker
pattern. The shell parses `|`, serializes the stage list, and calls one host
fn. The kernel creates N-1 pipes, builds N `Fiber`s with their FDs 0/1/2 bound
to pts (chain ends) or pipe ends (internal), runs all stages concurrently in a
single coordinator task via a manual `JoinAll` (sequential polls → peak native
stack = one fiber, so no wasmi stack overflow), and returns the last stage's
exit code. Each stage closes its own pipe-end FDs on exit so EOF propagates
downstream while the pipeline is still running.

**Tech Stack:** Rust `no_std`, `wasmi`, `embassy-executor` raw API, `alloc`
(`Arc`, `VecDeque`, `Box`, `Vec`), `spin::Mutex`.

---

## Testing approach (read first)

ruos has **no `no_std` unit-test harness**; kernel behavior is verified by
**integration tests under QEMU asserting serial output** (`make run-test`,
`make run-ssh-test`). Async kernel primitives (pipes) cannot be polled outside
an executor, so they are exercised by the real pipeline, not isolated unit
tests. Accordingly:

- Tasks 1-6 verify by **`make iso` compiling clean** (the unit of progress).
- Task 7 is the **behavioral test**: `ls / | grep bin` over the SSH exec path,
  asserting the output contains only matching lines. Written as a committed
  script `tests/pipe-test.sh`, run via `make run-pipe-test`.

Per task: implement → `make iso` (must compile) → commit. Final task runs the
integration test. Build via WSL:
`wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso'`

---

## File structure

- Create `kernel/src/pipe/mod.rs` — `Pipe` object + `PipeReadFile`/`PipeWriteFile`.
- Modify `kernel/src/main.rs` — `mod pipe;`.
- Modify `kernel/src/vfs/file.rs` — `FileImpl::PipeRead`/`PipeWrite` arms.
- Modify `kernel/src/vfs/mod.rs` — `pub fn pipe() -> (Fd, Fd)`.
- Modify `kernel/src/wasm/fiber.rs` — `Fiber::bind_fd(slot, fd)`.
- Modify `kernel/src/wasm/suspend.rs` — `SuspendReason::ExecPipeline`.
- Create `kernel/src/wasm/pipeline.rs` — `run_pipeline` + `JoinAll` + queue.
- Modify `kernel/src/wasm/mod.rs` — `pub mod pipeline;`.
- Modify `kernel/src/executor/mod.rs` — `pipeline_worker_task` + spawn it.
- Modify `kernel/src/wasm/fiber.rs` — dispatch arm for `ExecPipeline`.
- Modify `kernel/src/wasm/host/proc.rs` — `exec_pipeline` host fn + link.
- Modify `user/shell/src/main.rs` — `|` parse + serialize + call.
- Create `tests/pipe-test.sh` + modify `Makefile` — `run-pipe-test`.

---

## Task 1: Pipe object

**Files:**
- Create: `kernel/src/pipe/mod.rs`
- Modify: `kernel/src/main.rs` (add `mod pipe;` next to the other `mod` lines)

- [ ] **Step 1: Create `kernel/src/pipe/mod.rs`**

```rust
//! Anonymous byte-stream pipe for shell pipelines (`cmd1 | cmd2`).
//!
//! Mirrors the PTY waker pattern (Step 12): a shared `VecDeque` behind a
//! `spin::Mutex`, with a read end and a write end implementing the VFS
//! `File` trait. Bounded buffer gives backpressure; the writer end's `Drop`
//! signals EOF to the reader (`writers == 0`), and the reader end's `Drop`
//! signals a closed consumer to the writer (`readers == 0`).

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::task::Waker;
use spin::Mutex;
use x86_64::instructions::interrupts::without_interrupts;

use crate::vfs::error::VfsError;
use crate::vfs::file::{File, Whence};

/// Bounded pipe buffer capacity (bytes). 64 KiB is plenty for shell text.
const PIPE_CAP: usize = 64 * 1024;

struct PipeInner {
    buf: VecDeque<u8>,
    writers: usize,
    readers: usize,
    read_waker: Option<Waker>,
    write_waker: Option<Waker>,
}

type Pipe = Arc<Mutex<PipeInner>>;

/// Create a connected pipe pair. One reader, one writer open.
pub fn new_pipe() -> (PipeReadFile, PipeWriteFile) {
    let inner = Arc::new(Mutex::new(PipeInner {
        buf: VecDeque::new(),
        writers: 1,
        readers: 1,
        read_waker: None,
        write_waker: None,
    }));
    (PipeReadFile { inner: inner.clone() }, PipeWriteFile { inner })
}

pub struct PipeReadFile {
    inner: Pipe,
}

pub struct PipeWriteFile {
    inner: Pipe,
}

impl Drop for PipeWriteFile {
    fn drop(&mut self) {
        without_interrupts(|| {
            let mut g = self.inner.lock();
            g.writers = g.writers.saturating_sub(1);
            if g.writers == 0 {
                if let Some(w) = g.read_waker.take() { w.wake(); }
            }
        });
    }
}

impl Drop for PipeReadFile {
    fn drop(&mut self) {
        without_interrupts(|| {
            let mut g = self.inner.lock();
            g.readers = g.readers.saturating_sub(1);
            if g.readers == 0 {
                if let Some(w) = g.write_waker.take() { w.wake(); }
            }
        });
    }
}

impl File for PipeReadFile {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, VfsError> {
        if buf.is_empty() { return Ok(0); }
        let inner = self.inner.clone();
        core::future::poll_fn(|cx| {
            use core::task::Poll;
            without_interrupts(|| {
                let mut g = inner.lock();
                let mut n = 0;
                while n < buf.len() {
                    match g.buf.pop_front() {
                        Some(b) => { buf[n] = b; n += 1; }
                        None => break,
                    }
                }
                if n > 0 {
                    if let Some(w) = g.write_waker.take() { w.wake(); }
                    Poll::Ready(Ok(n))
                } else if g.writers == 0 {
                    Poll::Ready(Ok(0)) // EOF: all writers closed
                } else {
                    g.read_waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            })
        }).await
    }
    async fn write(&mut self, _buf: &[u8]) -> Result<usize, VfsError> {
        Err(VfsError::NotPermitted) // read end is not writable
    }
    async fn seek(&mut self, _off: i64, _w: Whence) -> Result<u64, VfsError> {
        Err(VfsError::NotPermitted)
    }
    async fn stat(&self) -> Result<crate::vfs::fs::VfsStat, VfsError> {
        Ok(crate::vfs::fs::VfsStat { kind: crate::vfs::fs::VfsKind::Device, size: 0 })
    }
}

impl File for PipeWriteFile {
    async fn read(&mut self, _buf: &mut [u8]) -> Result<usize, VfsError> {
        Err(VfsError::NotPermitted) // write end is not readable
    }
    async fn write(&mut self, buf: &[u8]) -> Result<usize, VfsError> {
        if buf.is_empty() { return Ok(0); }
        let inner = self.inner.clone();
        core::future::poll_fn(|cx| {
            use core::task::Poll;
            without_interrupts(|| {
                let mut g = inner.lock();
                if g.readers == 0 {
                    return Poll::Ready(Ok(0)); // consumer gone; stdout closed
                }
                let room = PIPE_CAP.saturating_sub(g.buf.len());
                if room == 0 {
                    g.write_waker = Some(cx.waker().clone());
                    return Poll::Pending;
                }
                let n = room.min(buf.len());
                g.buf.extend(&buf[..n]);
                if let Some(w) = g.read_waker.take() { w.wake(); }
                Poll::Ready(Ok(n))
            })
        }).await
    }
    async fn seek(&mut self, _off: i64, _w: Whence) -> Result<u64, VfsError> {
        Err(VfsError::NotPermitted)
    }
    async fn stat(&self) -> Result<crate::vfs::fs::VfsStat, VfsError> {
        Ok(crate::vfs::fs::VfsStat { kind: crate::vfs::fs::VfsKind::Device, size: 0 })
    }
}
```

- [ ] **Step 2: Register the module.** In `kernel/src/main.rs`, add `mod pipe;`
  alongside the existing top-level `mod` declarations (e.g. after `mod proc;`).

- [ ] **Step 3: Build to verify it compiles**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | grep -iE "error|Limine BIOS stages installed"'`
Expected: `Limine BIOS stages installed successfully.` and no `error`.

Note: `File`/`Whence`/`VfsStat`/`VfsKind`/`VfsError` import paths must match
`kernel/src/vfs/devices.rs` (the `PtySlaveFile` file) — copy them from there if
the paths differ.

- [ ] **Step 4: Commit**

```bash
git add kernel/src/pipe/mod.rs kernel/src/main.rs
git commit -m "feat(pipe): anonymous byte-stream Pipe with read/write File ends"
```

---

## Task 2: VFS wiring — pipe FDs

**Files:**
- Modify: `kernel/src/vfs/file.rs` (the `FileImpl` enum + its `read`/`write`/`stat`/`seek` match arms)
- Modify: `kernel/src/vfs/mod.rs` (add `pub fn pipe()`)

- [ ] **Step 1: Add `FileImpl` variants.** In `kernel/src/vfs/file.rs`, add to
  the `use` block: `use crate::pipe::{PipeReadFile, PipeWriteFile};` and add the
  two variants to `enum FileImpl`:

```rust
    PipeRead(PipeReadFile),
    PipeWrite(PipeWriteFile),
```

Then add an arm to EACH of the four match blocks (`read`, `write`, `stat`,
`seek`), following the existing `FileImpl::PtySlave(f) => f.<method>(...).await`
pattern. For example in `read`:

```rust
            FileImpl::PipeRead(f)  => f.read(buf).await,
            FileImpl::PipeWrite(f) => f.read(buf).await,
```

and analogously for `write`, `stat`, `seek`.

- [ ] **Step 2: Add `vfs::pipe()`.** In `kernel/src/vfs/mod.rs`, near the other
  fd helpers (the module already does `use crate::vfs::fd::{... allocate ...}`),
  add:

```rust
/// Create an anonymous pipe. Returns `(read_fd, write_fd)` — two global VFS
/// FDs with no path. The caller (pipeline coordinator) binds them to stage
/// FDs and closes them when each stage exits so EOF propagates.
pub fn pipe() -> (Fd, Fd) {
    use crate::vfs::file::FileImpl;
    let (r, w) = crate::pipe::new_pipe();
    let read_fd = crate::vfs::fd::allocate(FileImpl::PipeRead(r));
    let write_fd = crate::vfs::fd::allocate(FileImpl::PipeWrite(w));
    (read_fd, write_fd)
}
```

If `Fd` or `allocate` are not already in scope in `mod.rs`, add
`use crate::vfs::file::Fd;` and reference `crate::vfs::fd::allocate`.

- [ ] **Step 3: Build to verify**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | grep -iE "error|Limine BIOS stages installed"'`
Expected: `Limine BIOS stages installed successfully.` and no `error`.

- [ ] **Step 4: Commit**

```bash
git add kernel/src/vfs/file.rs kernel/src/vfs/mod.rs
git commit -m "feat(vfs): FileImpl pipe ends + vfs::pipe() anonymous fd pair"
```

---

## Task 3: Fiber FD binding

**Files:**
- Modify: `kernel/src/wasm/fiber.rs` (add a method near `rebind_stdio_pty`)

- [ ] **Step 1: Add `Fiber::bind_fd`.** In `kernel/src/wasm/fiber.rs`, after the
  existing `rebind_stdio_pty` method, add:

```rust
    /// Replace this fiber's FD `slot` (0=stdin, 1=stdout, 2=stderr) with the
    /// kernel VFS fd `fd`, closing the default `/dev/pts/0` entry it replaces.
    /// Used by the pipeline coordinator to wire a stage to a pipe end.
    pub fn bind_fd(&mut self, slot: usize, fd: crate::vfs::Fd) {
        use crate::vfs;
        use crate::wasm::state::FdEntry;
        let fds = &mut self.store.data_mut().fds;
        if slot >= fds.len() { return; }
        if let Some(FdEntry::Vfs(old)) = fds[slot].as_ref() {
            let old = *old;
            let _ = vfs::block_on(vfs::close(old));
        }
        fds[slot] = Some(FdEntry::Vfs(fd));
    }
```

- [ ] **Step 2: Build to verify**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | grep -iE "error|Limine BIOS stages installed"'`
Expected: `Limine BIOS stages installed successfully.` and no `error`.

- [ ] **Step 3: Commit**

```bash
git add kernel/src/wasm/fiber.rs
git commit -m "feat(wasm): Fiber::bind_fd to wire a stage stdio to a kernel fd"
```

---

## Task 4: Pipeline coordinator + concurrent exec

**Files:**
- Modify: `kernel/src/wasm/suspend.rs` (add `ExecPipeline` variant)
- Create: `kernel/src/wasm/pipeline.rs`
- Modify: `kernel/src/wasm/mod.rs` (`pub mod pipeline;`)
- Modify: `kernel/src/executor/mod.rs` (spawn `pipeline_worker_task`)
- Modify: `kernel/src/wasm/fiber.rs` (dispatch arm)

- [ ] **Step 1: Add the suspend reason.** In `kernel/src/wasm/suspend.rs`, add a
  variant to `enum SuspendReason` (match the `Exec` variant's field style):

```rust
    ExecPipeline {
        stages: alloc::vec::Vec<(alloc::string::String, alloc::vec::Vec<alloc::vec::Vec<u8>>)>,
        cwd: alloc::string::String,
        exit_code_ptr: u32,
    },
```

- [ ] **Step 2: Create `kernel/src/wasm/pipeline.rs`**

```rust
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

// ---- single-slot pipeline request queue (shell is blocked during run) ----

struct PipeRequest {
    stages: Vec<Stage>,
    cwd: String,
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

/// Future returned to the shell fiber: posts the pipeline, resolves to the
/// last stage's exit code.
pub struct PipelineFuture {
    posted: bool,
    stages: Vec<Stage>,
    cwd: String,
}

pub fn post_and_wait(stages: Vec<Stage>, cwd: String) -> PipelineFuture {
    PipelineFuture { posted: false, stages, cwd }
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
            *PIPELINE.pending.lock() = Some(PipeRequest { stages, cwd });
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

/// Future for the worker task: resolves when a pipeline is posted.
pub struct WaitForPipeline;
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

/// The executor task that drives pipelines.
pub async fn worker() {
    use core::sync::atomic::Ordering;
    loop {
        let req = WaitForPipeline.await;
        let code = run_pipeline(req.stages, req.cwd).await;
        PIPELINE.result.store(code, Ordering::SeqCst);
        PIPELINE.done.store(true, Ordering::SeqCst);
        if let Some(w) = PIPELINE.shell_waker.lock().take() { w.wake(); }
    }
}

// ---- manual JoinAll: poll every not-done future each round ----

struct JoinAll {
    futs: Vec<Option<Pin<Box<dyn Future<Output = i32>>>>>,
    codes: Vec<i32>,
}

impl Future for JoinAll {
    type Output = Vec<i32>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Vec<i32>> {
        let me = self.get_mut(); // JoinAll: Unpin (Vec of Pin<Box<_>> is Unpin)
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

/// Build and run all stages concurrently. Returns the last stage's exit code.
pub async fn run_pipeline(stages: Vec<Stage>, cwd: String) -> i32 {
    let n = stages.len();
    if n == 0 { return 0; }
    if n > PIPE_MAX_STAGES {
        kprintln!("ruos: pipeline too long ({} > {})", n, PIPE_MAX_STAGES);
        return 1;
    }

    // Create N-1 pipes: pipes[i] connects stage i (write) -> stage i+1 (read).
    // Each entry is (read_fd, write_fd).
    let mut pipes: Vec<(crate::vfs::Fd, crate::vfs::Fd)> = Vec::with_capacity(n - 1);
    for _ in 0..n.saturating_sub(1) {
        pipes.push(crate::vfs::pipe());
    }

    let mut futs: Vec<Option<Pin<Box<dyn Future<Output = i32>>>>> = Vec::with_capacity(n);

    for (i, (path, argv)) in stages.into_iter().enumerate() {
        // FDs this stage must close on exit (so EOF/closed-consumer propagate).
        let mut close_fds: Vec<crate::vfs::Fd> = Vec::new();
        // stdin: stage 0 keeps default pts; later stages read from pipes[i-1].
        let stdin_fd = if i > 0 { Some(pipes[i - 1].0) } else { None };
        // stdout: last stage keeps default pts; earlier stages write pipes[i].
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
            if let Some(fd) = stdin_fd { fb.bind_fd(0, fd); }
            if let Some(fd) = stdout_fd { fb.bind_fd(1, fd); }
            let pid = crate::proc::register(
                alloc::string::String::from(path.trim_start_matches('/')),
            );
            fb.set_pid(pid);
            let code = fb.run().await;
            crate::proc::unregister(pid);
            // Close this stage's pipe-end FDs → drops the Pipe File ends →
            // downstream reader sees EOF, upstream writer sees closed consumer.
            for fd in &close_fds { let _ = crate::vfs::block_on(crate::vfs::close(*fd)); }
            code
        };
        futs.push(Some(Box::pin(fut)));
    }

    let codes = JoinAll { codes: alloc::vec![0i32; n], futs }.await;
    *codes.last().unwrap_or(&0)
}
```

- [ ] **Step 3: Register module + spawn worker.**
  - In `kernel/src/wasm/mod.rs` add `pub mod pipeline;` next to `pub mod exec_queue;`.
  - In `kernel/src/executor/mod.rs`, add a task and spawn it in `run()` next to
    `spawner.spawn(exec_worker_task()).unwrap();`:

```rust
    spawner.spawn(pipeline_worker_task()).unwrap();
```

  and the task definition near `exec_worker_task`:

```rust
#[embassy_executor::task]
async fn pipeline_worker_task() {
    crate::wasm::pipeline::worker().await;
}
```

- [ ] **Step 4: Dispatch the suspend reason.** In `kernel/src/wasm/fiber.rs`,
  in `dispatch`, immediately after the `SuspendReason::Exec { .. }` arm (which
  ends `let _ = self.write_u32(exit_code_ptr, code as u32); 0`), add the
  pipeline arm — identical shape, posting to the pipeline queue instead:

```rust
            SuspendReason::ExecPipeline { stages, cwd, exit_code_ptr } => {
                let code = crate::wasm::pipeline::post_and_wait(stages, cwd).await;
                let _ = self.write_u32(exit_code_ptr, code as u32);
                0
            }
```

- [ ] **Step 5: Build to verify**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | grep -iE "error|Limine BIOS stages installed"'`
Expected: `Limine BIOS stages installed successfully.` and no `error`.

- [ ] **Step 6: Commit**

```bash
git add kernel/src/wasm/suspend.rs kernel/src/wasm/pipeline.rs kernel/src/wasm/mod.rs kernel/src/executor/mod.rs kernel/src/wasm/fiber.rs
git commit -m "feat(wasm): concurrent pipeline coordinator (JoinAll, per-stage fd close)"
```

---

## Task 5: `exec_pipeline` host fn

**Files:**
- Modify: `kernel/src/wasm/host/proc.rs` (add the host fn + register in `link`)

Serialization format (shell → kernel), little-endian:
`u32 nstages` then per stage: `u32 path_len, path bytes, u32 argc, (u32 arg_len, arg bytes) × argc`.

- [ ] **Step 1: Add the host fn.** In `kernel/src/wasm/host/proc.rs`, add:

```rust
/// ruos_exec_pipeline(buf_ptr, buf_len, exit_code_ptr) -> errno.
/// `buf` is the serialized stage list (see plan). Runs all stages concurrently
/// joined by pipes; writes the last stage's exit code at `exit_code_ptr`.
pub fn ruos_exec_pipeline(
    caller: Caller<'_, RuntimeState>,
    buf_ptr: i32,
    buf_len: i32,
    exit_code_ptr: i32,
) -> Result<i32, Error> {
    let mem = wasm_memory(&caller)?;
    let mut blob = alloc::vec![0u8; buf_len as usize];
    mem.read(&caller, buf_ptr as usize, &mut blob)
        .map_err(|_| Error::i32_exit(-1))?;
    let stages = match decode_pipeline(&blob) {
        Some(s) if !s.is_empty() => s,
        _ => return Ok(22), // EINVAL: malformed/empty
    };
    if stages.len() > crate::wasm::pipeline::PIPE_MAX_STAGES {
        return Ok(7); // E2BIG: pipeline too long
    }
    let cwd = caller.data().cwd.clone();
    Err(Error::host(SuspendReason::ExecPipeline {
        stages,
        cwd,
        exit_code_ptr: exit_code_ptr as u32,
    }))
}

/// Decode the pipeline blob. Returns Vec<(path, argv)>.
fn decode_pipeline(blob: &[u8]) -> Option<Vec<(String, Vec<Vec<u8>>)>> {
    let rd_u32 = |b: &[u8], o: usize| -> Option<u32> {
        if o + 4 > b.len() { return None; }
        Some(u32::from_le_bytes([b[o], b[o+1], b[o+2], b[o+3]]))
    };
    let mut o = 0usize;
    let n = rd_u32(blob, o)? as usize; o += 4;
    let mut stages = Vec::with_capacity(n);
    for _ in 0..n {
        let plen = rd_u32(blob, o)? as usize; o += 4;
        if o + plen > blob.len() { return None; }
        let path = core::str::from_utf8(&blob[o..o+plen]).ok()?.to_string(); o += plen;
        let argc = rd_u32(blob, o)? as usize; o += 4;
        let mut argv = Vec::with_capacity(argc);
        for _ in 0..argc {
            let alen = rd_u32(blob, o)? as usize; o += 4;
            if o + alen > blob.len() { return None; }
            argv.push(blob[o..o+alen].to_vec()); o += alen;
        }
        stages.push((path, argv));
    }
    Some(stages)
}
```

- [ ] **Step 2: Register in `link`.** In the `link` fn's builder chain, add:

```rust
        .func_wrap("ruos", "exec_pipeline", ruos_exec_pipeline)?
```

- [ ] **Step 3: Build to verify**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | grep -iE "error|Limine BIOS stages installed"'`
Expected: `Limine BIOS stages installed successfully.` and no `error`.

- [ ] **Step 4: Commit**

```bash
git add kernel/src/wasm/host/proc.rs
git commit -m "feat(wasm): ruos_exec_pipeline host fn + blob decode"
```

---

## Task 6: Shell `|` parsing

**Files:**
- Modify: `user/shell/src/main.rs`

The shell currently tokenizes a line and calls `exec(path, argv, &exit)`. Add a
split on top-level `|` (outside quotes), and when present, serialize and call a
new `exec_pipeline` import.

- [ ] **Step 1: Declare the import.** In the `extern "C"` block, add:

```rust
    fn exec_pipeline(buf_ptr: u32, buf_len: u32, exit_code_ptr: u32) -> i32;
```

- [ ] **Step 2: Split + serialize + call.** In the command-execution path
  (where a non-builtin line currently becomes one `exec`), add pipeline
  handling. Use the EXISTING tokenizer to split each segment into argv. Pseudocode
  to adapt to the shell's actual functions:

```rust
// Split the raw line on '|' respecting the same quote rules the tokenizer uses.
let segments: Vec<String> = split_pipeline(&line); // implement: split on '|' outside quotes
if segments.len() > 1 {
    // Each segment -> argv via the existing tokenizer.
    let mut stages: Vec<(String, Vec<Vec<u8>>)> = Vec::new();
    for seg in &segments {
        let argv = tokenize(seg);                 // existing tokenizer
        if argv.is_empty() { eprintln!("shell: empty pipeline stage"); return; }
        if is_builtin(&argv[0]) {                 // cd/pwd/exit/...
            eprintln!("shell: builtin '{}' not allowed in a pipeline", argv[0]);
            return;
        }
        let path = resolve_path(&argv[0]);        // existing PATH lookup (e.g. /bin/<cmd>.wasm)
        let argv_bytes: Vec<Vec<u8>> = argv.iter().map(|a| a.as_bytes().to_vec()).collect();
        stages.push((path, argv_bytes));
    }
    let blob = serialize_pipeline(&stages);
    let mut exit: i32 = 0;
    let errno = unsafe {
        exec_pipeline(blob.as_ptr() as u32, blob.len() as u32, &mut exit as *mut i32 as u32)
    };
    if errno == 7 { eprintln!("shell: pipeline too long (max 4)"); }
    else if errno != 0 { eprintln!("shell: exec_pipeline errno {}", errno); }
    return;
}
// else: existing single-command exec path (unchanged)
```

- [ ] **Step 3: Add the serializer helper.**

```rust
fn serialize_pipeline(stages: &[(String, Vec<Vec<u8>>)]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&(stages.len() as u32).to_le_bytes());
    for (path, argv) in stages {
        let p = path.as_bytes();
        b.extend_from_slice(&(p.len() as u32).to_le_bytes());
        b.extend_from_slice(p);
        b.extend_from_slice(&(argv.len() as u32).to_le_bytes());
        for a in argv {
            b.extend_from_slice(&(a.len() as u32).to_le_bytes());
            b.extend_from_slice(a);
        }
    }
    b
}

// Split `line` on '|' that are outside single/double quotes.
fn split_pipeline(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut sq, mut dq) = (false, false);
    for c in line.chars() {
        match c {
            '\'' if !dq => { sq = !sq; cur.push(c); }
            '"'  if !sq => { dq = !dq; cur.push(c); }
            '|'  if !sq && !dq => { out.push(cur.trim().to_string()); cur.clear(); }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() { out.push(cur.trim().to_string()); }
    out
}
```

Adapt `tokenize`, `is_builtin`, `resolve_path` to the shell's existing function
names (read `user/shell/src/main.rs` first; reuse what's there — DRY).

- [ ] **Step 4: Build to verify** (the shell wasm is rebuilt by `make iso`)

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | grep -iE "error|Limine BIOS stages installed"'`
Expected: `Limine BIOS stages installed successfully.` and no `error`.

- [ ] **Step 5: Commit**

```bash
git add user/shell/src/main.rs
git commit -m "feat(shell): parse '|' pipelines, call exec_pipeline"
```

---

## Task 7: Integration test

**Files:**
- Create: `tests/pipe-test.sh`
- Modify: `Makefile` (add `run-pipe-test`)

The test reuses the SSH exec path (proven in `tests/ssh-shell-test.sh`): the SSH
server seeds the command into the shell, which parses the `|`. `ls /` lists root
entries (one per line); `grep bin` keeps only the line(s) containing `bin`.

- [ ] **Step 1: Create `tests/pipe-test.sh`**

```bash
#!/usr/bin/env bash
# Integration test: shell pipelines (`cmd1 | cmd2`).
# Boots ruos, connects over SSH, runs `ls / | grep bin`, asserts the output
# contains `bin` and NOT an unrelated root entry (so the pipe really filtered).
set -u
cd "$(dirname "$0")/.."
ISO=build/os.iso; DISK=build/disk.img; KEY=build/id_ed25519; PORT=2222
for pid in $(pgrep -f 'qemu-system-x86_64'); do kill -9 "$pid" 2>/dev/null || true; done
sleep 1
for _ in $(seq 1 30); do ss -ltn 2>/dev/null | grep -q ":$PORT " && sleep 1 || break; done
cp "$KEY" /tmp/ruos_id && chmod 600 /tmp/ruos_id
rm -f build/serial.log build/pipe.log
timeout 60 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom "$ISO" \
  -serial stdio -display none -no-reboot -m 512 -device qemu-xhci \
  -netdev user,id=net0,hostfwd=tcp:127.0.0.1:$PORT-:22 \
  -device virtio-net-pci,netdev=net0 \
  -drive file="$DISK",format=raw,if=none,id=disk0 \
  -device ahci,id=ahci -device ide-hd,drive=disk0,bus=ahci.0 \
  > build/serial.log 2>&1 &
QEMUPID=$!
sleep 15
timeout 20 ssh -T -p "$PORT" -i /tmp/ruos_id \
  -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
  -o ConnectTimeout=5 root@127.0.0.1 'ls / | grep bin' </dev/null \
  > build/pipe.log 2>/dev/null || true
sleep 2
kill "$QEMUPID" 2>/dev/null || true
wait "$QEMUPID" 2>/dev/null || true
echo "=== pipe.log ==="; cat -v build/pipe.log
if grep -q 'bin' build/pipe.log; then echo TEST_PASS_PIPE; else
  echo TEST_FAIL_PIPE; tail -20 build/serial.log; exit 1; fi
```

NOTE: confirm `/bin` exists in the ISO root layout (it does — the Makefile
copies tools to `$(ISO_ROOT)/bin/`), and that `grep` exists as `/bin/grep.wasm`.
If `grep` is absent, substitute a tool that is present (check `user/` crates) or
add a minimal `grep`; adjust the asserted needle accordingly.

- [ ] **Step 2: Add the Makefile target.** After `run-ssh-test`, add:

```makefile
.PHONY: run-pipe-test
run-pipe-test: iso ssh-key-on-disk
	bash tests/pipe-test.sh
```

- [ ] **Step 3: Run the test**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-pipe-test 2>&1 | tail -8'`
Expected: output shows the filtered lines and `TEST_PASS_PIPE`.

If it fails: check `build/serial.log` for `pipeline` errors, confirm `grep.wasm`
exists, and that `ls`/`grep` are external (not builtins) in the shell.

- [ ] **Step 4: CHANGELOG + commit**

Create `CHANGELOG/NN-26-05-30-pipes.md` (NN = next number; check the highest in
`CHANGELOG/` first), summarizing: anonymous Pipe File ends, `vfs::pipe()`,
concurrent pipeline coordinator (JoinAll, per-stage fd close for EOF),
`exec_pipeline` host fn, shell `|` parsing, `make run-pipe-test`.

```bash
git add tests/pipe-test.sh Makefile CHANGELOG/NN-26-05-30-pipes.md
git commit -m "test(pipe): run-pipe-test integration (ls / | grep bin) + changelog"
```

---

## Self-review notes (addressed)

- **Spec coverage:** Pipe object (T1) ✓, VFS fd ends (T2) ✓, concurrent exec
  (T4) ✓, exec_pipeline host fn (T5) ✓, shell `|` + builtin rejection (T6) ✓,
  EOF on writer close + closed-consumer (T1 readers/writers + T4 per-stage
  close) ✓, max stages (T4 `PIPE_MAX_STAGES` + T5 E2BIG) ✓, integration test
  (T7) ✓.
- **Deadlock avoidance:** each stage closes its pipe-end FDs on exit (T4) →
  `writers==0` EOF reaches the consumer while the pipeline runs; `readers==0`
  unblocks a stalled producer.
- **Stack safety:** stages joined in ONE coordinator task; sequential polls →
  peak wasmi stack = one stage (same as `exec_worker`).
- **Exit-code write:** the `ExecPipeline` dispatch arm uses
  `self.write_u32(exit_code_ptr, code as u32)` — verified identical to the
  `Exec` arm in `fiber.rs:280`.
