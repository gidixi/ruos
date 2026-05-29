//! Device files: console (serial), null, zero.

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
