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

extern crate alloc;
use alloc::boxed::Box;

/// A `BlockDevice` view of one partition: every LBA is offset by `base`, and
/// the device length is clamped to `count` sectors. Lets the FAT32 driver mount
/// a partition unchanged (it reads "LBA 0" = the partition's first sector).
pub struct PartitionDevice {
    inner: Box<dyn BlockDevice + Send>,
    base: u64,
    count: u64,
}

impl PartitionDevice {
    pub fn new(inner: Box<dyn BlockDevice + Send>, base: u64, count: u64) -> Self {
        Self { inner, base, count }
    }
}

impl BlockDevice for PartitionDevice {
    fn block_size(&self) -> u32 { self.inner.block_size() }
    fn block_count(&self) -> u64 { self.count }
    fn read_blocks(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        let n = (buf.len() as u64) / self.inner.block_size() as u64;
        if lba.checked_add(n).map_or(true, |end| end > self.count) {
            return Err(BlockError::OutOfRange);
        }
        let phys = self.base.checked_add(lba).ok_or(BlockError::OutOfRange)?;
        self.inner.read_blocks(phys, buf)
    }
    fn write_blocks(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        let n = (buf.len() as u64) / self.inner.block_size() as u64;
        if lba.checked_add(n).map_or(true, |end| end > self.count) {
            return Err(BlockError::OutOfRange);
        }
        let phys = self.base.checked_add(lba).ok_or(BlockError::OutOfRange)?;
        self.inner.write_blocks(phys, buf)
    }
}

/// A borrowing partition view: like [`PartitionDevice`] but holds a `&mut dyn
/// BlockDevice` instead of owning a `Box`. Every LBA is offset by `base`, and
/// the device length is clamped to `count` sectors. Used by the disk-authoring
/// path (`disk::author`), which only has a borrow of the raw disk yet must
/// format + create dirs on individual partition regions of it. The range/offset
/// checks are identical to `PartitionDevice` — the partition-isolation boundary
/// holds the same way.
pub struct PartBorrow<'a> {
    inner: &'a mut dyn BlockDevice,
    base: u64,
    count: u64,
}

impl<'a> PartBorrow<'a> {
    pub fn new(inner: &'a mut dyn BlockDevice, base: u64, count: u64) -> Self {
        Self { inner, base, count }
    }
}

impl<'a> BlockDevice for PartBorrow<'a> {
    fn block_size(&self) -> u32 { self.inner.block_size() }
    fn block_count(&self) -> u64 { self.count }
    fn read_blocks(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        let n = (buf.len() as u64) / self.inner.block_size() as u64;
        if lba.checked_add(n).map_or(true, |end| end > self.count) {
            return Err(BlockError::OutOfRange);
        }
        let phys = self.base.checked_add(lba).ok_or(BlockError::OutOfRange)?;
        self.inner.read_blocks(phys, buf)
    }
    fn write_blocks(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        let n = (buf.len() as u64) / self.inner.block_size() as u64;
        if lba.checked_add(n).map_or(true, |end| end > self.count) {
            return Err(BlockError::OutOfRange);
        }
        let phys = self.base.checked_add(lba).ok_or(BlockError::OutOfRange)?;
        self.inner.write_blocks(phys, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*; extern crate std; use std::vec; use std::vec::Vec;
    struct Mem(Vec<u8>);
    impl BlockDevice for Mem {
        fn block_size(&self)->u32{512}
        fn block_count(&self)->u64{(self.0.len()/512) as u64}
        fn read_blocks(&mut self,lba:u64,buf:&mut[u8])->Result<(),BlockError>{
            let o=(lba as usize)*512; buf.copy_from_slice(&self.0[o..o+buf.len()]); Ok(())
        }
        fn write_blocks(&mut self,lba:u64,buf:&[u8])->Result<(),BlockError>{
            let o=(lba as usize)*512; self.0[o..o+buf.len()].copy_from_slice(buf); Ok(())
        }
    }
    #[test] fn offsets_and_clamps() {
        let mut backing = vec![0u8; 512*10];
        backing[5*512] = 0xAB;
        let mut pd = PartitionDevice::new(Box::new(Mem(backing)), 5, 3);
        let mut buf = [0u8;512];
        pd.read_blocks(0, &mut buf).unwrap();
        assert_eq!(buf[0], 0xAB);
        assert!(pd.read_blocks(3, &mut buf).is_err());
        assert_eq!(pd.block_count(), 3);
    }
}
