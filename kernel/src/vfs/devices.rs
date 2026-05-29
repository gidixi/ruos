//! Device files: console (serial), null, zero, pty-slave.

use crate::vfs::error::VfsError;
use crate::vfs::file::{File, Whence};
use core::fmt::Write as _;

pub struct ConsoleFile;
pub struct NullFile;
pub struct ZeroFile;

impl File for ConsoleFile {
    /// Reads one byte from the global keyboard queue, blocking until a
    /// char arrives. The queue is single-consumer; if any other code
    /// path (e.g. the legacy `SuspendReason::KbdReadChar` route via
    /// `FdEntry::Stdin`) drains the queue concurrently, reads here will
    /// race. Step 11 fixes this by making the shell the sole keyboard
    /// consumer (via /dev/console as stdin).
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, VfsError> {
        if buf.is_empty() { return Ok(0); }
        let b = crate::keyboard::queue::read_char().await;
        buf[0] = b;
        Ok(1)
    }
    async fn write(&mut self, buf: &[u8]) -> Result<usize, VfsError> {
        let mut serial = crate::serial::SERIAL.lock();
        for b in buf {
            // Write each byte as a 1-char str. Non-UTF-8 bytes print as '?'.
            let _ = serial.write_str(core::str::from_utf8(&[*b]).unwrap_or("?"));
        }
        Ok(buf.len())
    }
    async fn seek(&mut self, _off: i64, _w: Whence) -> Result<u64, VfsError> {
        Err(VfsError::NotPermitted)
    }
}

impl File for NullFile {
    async fn read(&mut self, _buf: &mut [u8]) -> Result<usize, VfsError> { Ok(0) }
    async fn write(&mut self, buf: &[u8]) -> Result<usize, VfsError> { Ok(buf.len()) }
    async fn seek(&mut self, _off: i64, _w: Whence) -> Result<u64, VfsError> { Ok(0) }
}

impl File for ZeroFile {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, VfsError> {
        for b in buf.iter_mut() { *b = 0; }
        Ok(buf.len())
    }
    async fn write(&mut self, buf: &[u8]) -> Result<usize, VfsError> { Ok(buf.len()) }
    async fn seek(&mut self, _off: i64, _w: Whence) -> Result<u64, VfsError> { Ok(0) }
}

/// A file handle for one PTY slave endpoint. Reads block (async) until the
/// line discipline delivers bytes into `slave_rx`; writes push bytes through
/// `process_output` (which handles ONLCR etc.) into `master_out`.
pub struct PtySlaveFile {
    pub idx: usize,
}

impl File for PtySlaveFile {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, VfsError> {
        if buf.is_empty() { return Ok(0); }
        let idx = self.idx;
        core::future::poll_fn(|cx| {
            use x86_64::instructions::interrupts::without_interrupts;
            use core::task::Poll;
            without_interrupts(|| {
                let mut g = crate::pty::pair(idx).lock();
                let mut n = 0;
                while n < buf.len() {
                    match g.slave_rx.pop_front() {
                        Some(b) => { buf[n] = b; n += 1; }
                        None => break,
                    }
                }
                if n > 0 {
                    Poll::Ready(Ok(n))
                } else {
                    g.slave_waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            })
        }).await
    }

    async fn write(&mut self, buf: &[u8]) -> Result<usize, VfsError> {
        if buf.is_empty() { return Ok(0); }
        let idx = self.idx;
        x86_64::instructions::interrupts::without_interrupts(|| {
            let mut g = crate::pty::pair(idx).lock();
            for &b in buf {
                crate::pty::ldisc::process_output(&mut g, b);
            }
        });
        Ok(buf.len())
    }

    async fn seek(&mut self, _off: i64, _w: Whence) -> Result<u64, VfsError> {
        Err(VfsError::NotPermitted)
    }
}
