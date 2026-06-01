//! Device files: console (serial), null, zero, pty-slave.

use crate::vfs::error::VfsError;
use crate::vfs::file::{File, Whence};
use core::fmt::Write as _;

pub struct ConsoleFile;
pub struct NullFile;
pub struct ZeroFile;

impl File for ConsoleFile {
    /// `/dev/console` read returns EOF. Shell stdin lives at `/dev/pts/0`
    /// (PTY slave) since Step 12. Reading from `/dev/console` would
    /// previously have drained the master_output queue (Step 12 T3
    /// implementer's workaround), feeding the shell its own stdout —
    /// loop hazard. Step 12 F1: stub to EOF; semantics for /dev/console
    /// will be revisited if/when a real device emerges.
    async fn read(&mut self, _buf: &mut [u8]) -> Result<usize, VfsError> {
        Ok(0)
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
    async fn stat(&self) -> Result<crate::vfs::fs::VfsStat, VfsError> {
        Ok(crate::vfs::fs::VfsStat { kind: crate::vfs::fs::VfsKind::Device, size: 0 })
    }
}

impl File for NullFile {
    async fn read(&mut self, _buf: &mut [u8]) -> Result<usize, VfsError> { Ok(0) }
    async fn write(&mut self, buf: &[u8]) -> Result<usize, VfsError> { Ok(buf.len()) }
    async fn seek(&mut self, _off: i64, _w: Whence) -> Result<u64, VfsError> { Ok(0) }
    async fn stat(&self) -> Result<crate::vfs::fs::VfsStat, VfsError> {
        Ok(crate::vfs::fs::VfsStat { kind: crate::vfs::fs::VfsKind::Device, size: 0 })
    }
}

impl File for ZeroFile {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, VfsError> {
        for b in buf.iter_mut() { *b = 0; }
        Ok(buf.len())
    }
    async fn write(&mut self, buf: &[u8]) -> Result<usize, VfsError> { Ok(buf.len()) }
    async fn seek(&mut self, _off: i64, _w: Whence) -> Result<u64, VfsError> { Ok(0) }
    async fn stat(&self) -> Result<crate::vfs::fs::VfsStat, VfsError> {
        Ok(crate::vfs::fs::VfsStat { kind: crate::vfs::fs::VfsKind::Device, size: 0 })
    }
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
        let n = core::future::poll_fn(|cx| {
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
                    Poll::Ready(Ok::<usize, VfsError>(n))
                } else if crate::pty::is_shutdown(idx)
                    || g.foreground_pid.map(|p| crate::proc::is_kill_pending(p)).unwrap_or(false)
                {
                    // Pair was hung up (SSH session dropped / watchdog idle
                    // timeout), OR the foreground app was `^C`'d / killed.
                    // Return EOF so a stdin-blocked app unwinds and exits
                    // (its fiber's post-dispatch kill check then fires).
                    Poll::Ready(Ok::<usize, VfsError>(0))
                } else {
                    g.slave_waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            })
        }).await?;
        if n > 0 { crate::pty::touch_activity(idx); }
        Ok(n)
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
        crate::pty::touch_activity(idx);
        Ok(buf.len())
    }

    async fn seek(&mut self, _off: i64, _w: Whence) -> Result<u64, VfsError> {
        Err(VfsError::NotPermitted)
    }
    async fn stat(&self) -> Result<crate::vfs::fs::VfsStat, VfsError> {
        Ok(crate::vfs::fs::VfsStat { kind: crate::vfs::fs::VfsKind::Device, size: 0 })
    }
}
