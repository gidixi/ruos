//! Kernel log ring buffer.
//!
//! Every `kprintln!` line is appended here in addition to the multi-console
//! sink. Userspace reads via the `ruos_dmesg_read` host fn (returns the
//! oldest-to-newest contents and advances no cursor — successive reads
//! return the same content unless trimmed). Fixed-size, no allocation
//! beyond the static buffer; oldest bytes overwrite when full.

use spin::Mutex;

const LOG_CAP: usize = 32 * 1024;

struct Ring {
    buf: [u8; LOG_CAP],
    head: usize, // next write position
    full: bool,  // true once buf has wrapped at least once
}

impl Ring {
    const fn new() -> Self {
        Self { buf: [0; LOG_CAP], head: 0, full: false }
    }

    fn push(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.buf[self.head] = b;
            self.head += 1;
            if self.head == LOG_CAP {
                self.head = 0;
                self.full = true;
            }
        }
    }

    /// Copy oldest-to-newest into `out`, return number of bytes written.
    fn read(&self, out: &mut [u8]) -> usize {
        if !self.full {
            let n = self.head.min(out.len());
            out[..n].copy_from_slice(&self.buf[..n]);
            n
        } else {
            // Wrapped: [head..end) is oldest, then [0..head) is newest.
            let tail_len = LOG_CAP - self.head;
            let total = LOG_CAP.min(out.len());
            let from_tail = tail_len.min(total);
            out[..from_tail].copy_from_slice(&self.buf[self.head..self.head + from_tail]);
            let remaining = total - from_tail;
            if remaining > 0 {
                out[from_tail..from_tail + remaining].copy_from_slice(&self.buf[..remaining]);
            }
            total
        }
    }
}

static LOG: Mutex<Ring> = Mutex::new(Ring::new());

pub fn push(bytes: &[u8]) {
    LOG.lock().push(bytes);
}

/// Best-effort push: appends `bytes` only if the ring lock is uncontested.
/// Safe to call from a panic handler where we must not block.
pub fn try_push(bytes: &[u8]) {
    if let Some(mut ring) = LOG.try_lock() {
        ring.push(bytes);
    }
}

pub fn read(out: &mut [u8]) -> usize {
    LOG.lock().read(out)
}

/// Fixed-size formatter sink: `kprintln!` / `binfo!` reformat into this so
/// the same bytes go to both console and klog without a double allocation.
/// Long messages clip silently — dmesg lines are typically short.
pub struct Scratch {
    buf: [u8; 256],
    len: usize,
}

impl Scratch {
    pub const fn new() -> Self {
        Self { buf: [0; 256], len: 0 }
    }
    pub fn as_bytes(&self) -> &[u8] { &self.buf[..self.len] }
}

impl core::fmt::Write for Scratch {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let room = self.buf.len() - self.len;
        let n = s.len().min(room);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}
