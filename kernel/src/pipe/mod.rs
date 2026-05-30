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
