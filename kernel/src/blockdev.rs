//! Block-device abstraction.
//!
//! A `BlockDevice` is a fixed-sector-size random-access storage backend.
//! AHCI ports implement it (Step 15); future NVMe / virtio-blk will too.
//!
//! Reads and writes operate on whole sectors. Buffers must be a multiple of
//! `block_size()` bytes; the requested LBA must be < `block_count()`.
//! Callers are responsible for splitting larger transfers if the underlying
//! device caps a single command (e.g. AHCI PRDT limits one PRDT-entry to
//! 4 MiB → 8192 sectors at 512 B per LBA).

use core::fmt;

#[derive(Debug, Clone, Copy)]
pub enum BlockError {
    /// Device reported an error (e.g. AHCI PxTFD.ERR set, ATA error register).
    Io,
    /// LBA + sector count would extend past the device.
    OutOfRange,
    /// Buffer length not a multiple of `block_size()`, or address not aligned.
    BadAlignment,
    /// Hardware completion did not fire within the polling window.
    Timeout,
}

impl fmt::Display for BlockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlockError::Io           => write!(f, "device I/O error"),
            BlockError::OutOfRange   => write!(f, "LBA out of range"),
            BlockError::BadAlignment => write!(f, "bad alignment"),
            BlockError::Timeout      => write!(f, "device timeout"),
        }
    }
}

pub trait BlockDevice {
    /// Sector size in bytes (almost always 512 on SATA / NVMe-as-512).
    fn block_size(&self) -> u32;

    /// Total number of sectors the device exposes.
    fn block_count(&self) -> u64;

    /// Read `buf.len() / block_size()` sectors starting at `lba` into `buf`.
    /// `buf.len()` must be a multiple of `block_size()`.
    fn read_blocks(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError>;

    /// Write `buf.len() / block_size()` sectors starting at `lba` from `buf`.
    /// `buf.len()` must be a multiple of `block_size()`.
    fn write_blocks(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError>;
}
