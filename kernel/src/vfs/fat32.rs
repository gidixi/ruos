//! Minimal in-tree FAT32 filesystem.
//!
//! Read + write. Long file names (LFN) are reconstructed on read (so tools with
//! long names on `/mnt/bin`, e.g. `uname.wasm`, are looked up by their full
//! name); short-name-only records still resolve by their 8.3 form. All lookup
//! keys are lowercase-normalised. The authoring path ([`FatWriter`]) writes LFN
//! runs. Single FAT mirror updated.
//!
//! Backed by any [`BlockDevice`]; we currently mount only on an `AhciPort`
//! taken from the global `PORT0` slot. The full FS state lives behind one
//! `spin::Mutex` so the existing async VFS API (single-CPU, cooperative
//! executor) is correct without per-file locking.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::boxed::Box;
use spin::Mutex;

use crate::ahci::AhciPort;
use crate::blockdev::{BlockDevice, BlockError};
use crate::vfs::error::VfsError;
use crate::vfs::file::{File, FileImpl, OpenFlags, Whence};
use crate::vfs::fs::{FileSystem, VfsDirent, VfsKind, VfsStat};

const SECTOR: usize = 512;

// Directory entry constants.
const DIR_ENTRY_SIZE: usize = 32;
const ATTR_DIRECTORY: u8 = 0x10;
const ATTR_VOLUME_ID: u8 = 0x08;
const ATTR_LFN:       u8 = 0x0F;
const ATTR_ARCHIVE:   u8 = 0x20;

// FAT entry end-of-chain marker (lower 28 bits).
const EOC: u32 = 0x0FFF_FFF8;

#[derive(Debug, Clone, Copy)]
struct Bpb {
    bytes_per_sec:   u16,
    sec_per_cluster: u8,
    rsvd_sec_cnt:    u16,
    num_fats:        u8,
    fat_sz32:        u32,
    root_clus:       u32,
    tot_sec32:       u32,
}

impl Bpb {
    fn parse(sec0: &[u8]) -> Result<Self, VfsError> {
        if sec0.len() < SECTOR { return Err(VfsError::IoError); }
        if u16::from_le_bytes([sec0[510], sec0[511]]) != 0xAA55 {
            return Err(VfsError::IoError);
        }
        let bytes_per_sec   = u16::from_le_bytes([sec0[0x0B], sec0[0x0C]]);
        let sec_per_cluster = sec0[0x0D];
        let rsvd_sec_cnt    = u16::from_le_bytes([sec0[0x0E], sec0[0x0F]]);
        let num_fats        = sec0[0x10];
        let fat_sz32        = u32::from_le_bytes([sec0[0x24], sec0[0x25], sec0[0x26], sec0[0x27]]);
        let root_clus       = u32::from_le_bytes([sec0[0x2C], sec0[0x2D], sec0[0x2E], sec0[0x2F]]);
        let tot_sec32       = u32::from_le_bytes([sec0[0x20], sec0[0x21], sec0[0x22], sec0[0x23]]);
        if bytes_per_sec != SECTOR as u16 { return Err(VfsError::IoError); }
        if !sec_per_cluster.is_power_of_two() { return Err(VfsError::IoError); }
        Ok(Self {
            bytes_per_sec, sec_per_cluster, rsvd_sec_cnt, num_fats,
            fat_sz32, root_clus, tot_sec32,
        })
    }

    #[inline] fn cluster_bytes(&self) -> usize {
        usize::from(self.sec_per_cluster) * SECTOR
    }
    /// First sector of the data region.
    #[inline] fn data_start_sector(&self) -> u64 {
        u64::from(self.rsvd_sec_cnt) + u64::from(self.num_fats) * u64::from(self.fat_sz32)
    }
    /// First sector of cluster `n` (cluster numbers start at 2).
    #[inline] fn cluster_sector(&self, n: u32) -> u64 {
        self.data_start_sector() + (n as u64 - 2) * u64::from(self.sec_per_cluster)
    }
    /// Sector + byte offset within sector of FAT entry for cluster `n`.
    #[inline] fn fat_entry_loc(&self, n: u32) -> (u64, usize) {
        let off = n as u64 * 4;
        let sec = u64::from(self.rsvd_sec_cnt) + off / SECTOR as u64;
        (sec, (off % SECTOR as u64) as usize)
    }
    /// Highest valid cluster index + 1 (data-cluster count + 2). Saturating so a
    /// corrupt BPB (data region past the volume) yields a small bound instead of
    /// underflowing.
    #[inline] fn max_cluster(&self) -> u32 {
        ((self.tot_sec32 as u64).saturating_sub(self.data_start_sector())
            / u64::from(self.sec_per_cluster)).saturating_add(2) as u32
    }
}

/// One open file's mutable state.
struct Inner {
    dev: Box<dyn BlockDevice + Send>,
    bpb: Bpb,
}

impl Inner {
    fn read_sector(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), VfsError> {
        self.dev.read_blocks(lba, buf).map_err(map_block_err)
    }
    fn write_sector(&mut self, lba: u64, buf: &[u8]) -> Result<(), VfsError> {
        self.dev.write_blocks(lba, buf).map_err(map_block_err)
    }

    /// Follow the FAT chain starting at `start`, return every cluster index.
    fn chain(&mut self, start: u32) -> Result<Vec<u32>, VfsError> {
        let mut out = Vec::new();
        let mut sec_buf = [0u8; SECTOR];
        let mut cur = start;
        let mut cached_sec: Option<u64> = None;
        // A valid chain can't exceed the cluster count; a longer walk means a
        // cyclic/corrupt FAT — bail instead of looping forever (heap-exhaust DoS).
        let max_clusters = self.bpb.max_cluster() as usize;
        while cur >= 2 && (cur & 0x0FFF_FFFF) < EOC {
            if out.len() >= max_clusters { return Err(VfsError::IoError); }
            out.push(cur);
            let (sec, off) = self.bpb.fat_entry_loc(cur);
            if cached_sec != Some(sec) {
                self.read_sector(sec, &mut sec_buf)?;
                cached_sec = Some(sec);
            }
            let entry = u32::from_le_bytes([
                sec_buf[off], sec_buf[off+1], sec_buf[off+2], sec_buf[off+3]
            ]) & 0x0FFF_FFFF;
            if entry == 0 { return Err(VfsError::IoError); }
            cur = entry;
        }
        Ok(out)
    }

    /// Read one whole cluster `n` into `out` (must be cluster_bytes).
    fn read_cluster(&mut self, n: u32, out: &mut [u8]) -> Result<(), VfsError> {
        let sectors = u64::from(self.bpb.sec_per_cluster);
        let sec = self.bpb.cluster_sector(n);
        let len = self.bpb.cluster_bytes();
        if out.len() < len { return Err(VfsError::IoError); }
        // BlockDevice.read_blocks reads exact multiple of 512.
        for s in 0..sectors {
            let sub = &mut out[(s as usize) * SECTOR..((s+1) as usize) * SECTOR];
            self.read_sector(sec + s, sub)?;
        }
        Ok(())
    }

    fn write_cluster(&mut self, n: u32, data: &[u8]) -> Result<(), VfsError> {
        let sectors = u64::from(self.bpb.sec_per_cluster);
        let sec = self.bpb.cluster_sector(n);
        for s in 0..sectors {
            let sub = &data[(s as usize) * SECTOR..((s+1) as usize) * SECTOR];
            self.write_sector(sec + s, sub)?;
        }
        Ok(())
    }

    /// Allocate one free cluster from the FAT (linear scan starting at 2).
    /// Returns the cluster index. The entry is set to EOC.
    fn alloc_cluster(&mut self) -> Result<u32, VfsError> {
        let mut sec_buf = [0u8; SECTOR];
        let max_cluster = self.bpb.max_cluster();
        for n in 2..max_cluster {
            let (sec, off) = self.bpb.fat_entry_loc(n);
            self.read_sector(sec, &mut sec_buf)?;
            let entry = u32::from_le_bytes([
                sec_buf[off], sec_buf[off+1], sec_buf[off+2], sec_buf[off+3]
            ]) & 0x0FFF_FFFF;
            if entry == 0 {
                // Mark as EOC.
                self.write_fat_entry(n, 0x0FFF_FFFF)?;
                // Zero the data cluster so stale bytes don't leak.
                let zero = alloc::vec![0u8; self.bpb.cluster_bytes()];
                self.write_cluster(n, &zero)?;
                return Ok(n);
            }
        }
        Err(VfsError::NoSpace)
    }

    /// Write a single FAT entry (only the lower 28 bits matter); mirrors to
    /// every FAT copy.
    fn write_fat_entry(&mut self, cluster: u32, value: u32) -> Result<(), VfsError> {
        let (sec0, off) = self.bpb.fat_entry_loc(cluster);
        let mut sec_buf = [0u8; SECTOR];
        for fat_idx in 0..u64::from(self.bpb.num_fats) {
            let sec = sec0 + fat_idx * u64::from(self.bpb.fat_sz32);
            self.read_sector(sec, &mut sec_buf)?;
            let mut e = u32::from_le_bytes([
                sec_buf[off], sec_buf[off+1], sec_buf[off+2], sec_buf[off+3]
            ]);
            e = (e & 0xF000_0000) | (value & 0x0FFF_FFFF);
            let b = e.to_le_bytes();
            sec_buf[off..off+4].copy_from_slice(&b);
            self.write_sector(sec, &sec_buf)?;
        }
        Ok(())
    }

    /// Find a free slot in directory `dir_cluster`'s chain and write `rec` (a
    /// 32-byte short record). If every cluster is full, allocate + link a new
    /// cluster to the dir's chain and use its first slot. Returns the on-disk
    /// `(rec_sec, rec_off)` of the written record.
    fn add_dir_record(&mut self, dir_cluster: u32, rec: &[u8; DIR_ENTRY_SIZE])
        -> Result<(u64, usize), VfsError>
    {
        let chain = self.chain(dir_cluster)?;
        let cluster_bytes = self.bpb.cluster_bytes();
        let entries_per_cluster = cluster_bytes / DIR_ENTRY_SIZE;
        let mut buf = alloc::vec![0u8; cluster_bytes];
        // Look for a free slot (0x00 = never used, 0xE5 = deleted) in an
        // existing cluster of the chain.
        for &c in &chain {
            self.read_cluster(c, &mut buf)?;
            for i in 0..entries_per_cluster {
                let first = buf[i * DIR_ENTRY_SIZE];
                if first == 0x00 || first == 0xE5 {
                    buf[i * DIR_ENTRY_SIZE..(i+1) * DIR_ENTRY_SIZE].copy_from_slice(rec);
                    self.write_cluster(c, &buf)?;
                    let rec_sec = self.bpb.cluster_sector(c) + (i * DIR_ENTRY_SIZE / SECTOR) as u64;
                    let rec_off = (i * DIR_ENTRY_SIZE) % SECTOR;
                    return Ok((rec_sec, rec_off));
                }
            }
        }
        // Every cluster is full — extend the chain. Allocate a fresh cluster
        // (already zeroed + set EOC by `alloc_cluster`), link the old tail to it,
        // and write the record at slot 0 of the new cluster. A zeroed cluster's
        // slot 0 has first byte 0x00 (free); leaving the rest zero terminates the
        // directory scan correctly.
        let last = *chain.last().ok_or(VfsError::IoError)?;
        let new_cluster = self.alloc_cluster()?;
        self.write_fat_entry(last, new_cluster)?;
        buf.iter_mut().for_each(|b| *b = 0);
        buf[0..DIR_ENTRY_SIZE].copy_from_slice(rec);
        self.write_cluster(new_cluster, &buf)?;
        let rec_sec = self.bpb.cluster_sector(new_cluster); // slot 0 → offset 0
        Ok((rec_sec, 0))
    }
}

fn map_block_err(_: BlockError) -> VfsError { VfsError::IoError }

/// Decoded directory entry on disk.
#[derive(Debug, Clone)]
struct DirEntry {
    /// Lowercased lookup key. The reconstructed LFN long name when the on-disk
    /// record is preceded by an LFN run (e.g. "uname.wasm"); otherwise the
    /// normalised 8.3 short name ("hello.txt").
    name:      String,
    is_dir:    bool,
    cluster:   u32,
    size:      u32,
    /// Sector containing the on-disk 32-byte record.
    rec_sec:   u64,
    /// Byte offset of the record within `rec_sec`.
    rec_off:   usize,
}

/// Read every file/dir entry inside the directory starting at `start_cluster`,
/// reconstructing **long file names** (LFN) where present.
///
/// Each real 8.3 record may be preceded by a run of `ATTR_LFN` sub-entries (the
/// mirror of [`FatWriter::build_lfn_run`]) that, decoded, spell the file's long
/// name. We accumulate those sub-entries into `lfn` indexed by their 1-based
/// ordinal (so the run is reconstructed correctly regardless of physical order,
/// though on disk it is stored highest-ordinal-first) and, on reaching the
/// following short entry, expose the reconstructed long name as `DirEntry.name`
/// (lowercased, matching the lookup convention). A short-name-only record (no
/// preceding LFN run, e.g. `HELLO.TXT`) keeps the 8.3 decode unchanged.
///
/// The LFN checksum (byte 13 of every sub-entry) is verified against the short
/// name's `ChkSum`; on any mismatch the run is discarded and we fall back to the
/// 8.3 name, so a torn/garbage LFN run can never surface a wrong name.
fn read_dir_entries(inner: &mut Inner, start_cluster: u32) -> Result<Vec<DirEntry>, VfsError> {
    let chain = inner.chain(start_cluster)?;
    let cluster_bytes = inner.bpb.cluster_bytes();
    let mut out = Vec::new();
    let mut buf = alloc::vec![0u8; cluster_bytes];
    // LFN reconstruction buffer: up to 20 sub-entries × 13 UTF-16 units. `lfn_max`
    // is the high-water mark (ordinal*13) of units written for the current run;
    // 0 means "no pending LFN run" → use the 8.3 short name. `lfn_csum` is the
    // checksum carried by the run's sub-entries (must match the short name's).
    let mut lfn: [u16; 260] = [0; 260];
    let mut lfn_max: usize = 0;
    let mut lfn_csum: u8 = 0;
    for c in chain {
        inner.read_cluster(c, &mut buf)?;
        let entries_per_cluster = cluster_bytes / DIR_ENTRY_SIZE;
        for i in 0..entries_per_cluster {
            let rec = &buf[i * DIR_ENTRY_SIZE..(i+1) * DIR_ENTRY_SIZE];
            let first = rec[0];
            if first == 0x00 { return Ok(out); }            // end of directory
            if first == 0xE5 { lfn_max = 0; continue; }     // deleted → drop run
            let attr = rec[11];
            if attr == ATTR_LFN {
                // LFN sub-entry: stash its 13 UTF-16LE units by ordinal.
                let ord = (rec[0] & 0x1F) as usize;        // 1-based, ignore 0x40
                if ord >= 1 && ord <= 20 {
                    // Fresh run (no pending units) → clear the buffer first, so a
                    // torn run with a missing middle ordinal truncates at the gap
                    // (decode stops at the 0x0000) instead of reading stale units
                    // left from a prior file's run.
                    if lfn_max == 0 { lfn = [0; 260]; }
                    let base = (ord - 1) * 13;
                    // Units live at offsets 1..11 (5), 14..26 (6), 28..32 (2).
                    const SLOTS: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
                    for (j, &off) in SLOTS.iter().enumerate() {
                        lfn[base + j] = u16::from_le_bytes([rec[off], rec[off + 1]]);
                    }
                    // Every sub-entry of a run carries the same short-name
                    // checksum; the guard at the short entry rejects a mismatch.
                    lfn_csum = rec[13];
                    lfn_max = lfn_max.max(ord * 13);
                } else {
                    lfn_max = 0;                            // bogus ordinal → drop
                }
                continue;                                   // never a DirEntry
            }
            if attr & ATTR_VOLUME_ID != 0 { lfn_max = 0; continue; } // volume label
            let cluster_hi = u16::from_le_bytes([rec[20], rec[21]]) as u32;
            let cluster_lo = u16::from_le_bytes([rec[26], rec[27]]) as u32;
            let cluster = (cluster_hi << 16) | cluster_lo;
            let size = u32::from_le_bytes([rec[28], rec[29], rec[30], rec[31]]);
            // Reconstruct the long name if a valid LFN run preceded this record
            // (checksum must match the short name); else use the 8.3 decode.
            let mut short11 = [0u8; 11];
            short11.copy_from_slice(&rec[0..11]);
            let name = if lfn_max > 0 && lfn_checksum_short(&short11) == lfn_csum {
                decode_long_name(&lfn[..lfn_max])
            } else {
                decode_short_name(&rec[0..11])
            };
            lfn_max = 0;                                    // consume the run
            if name.is_empty() { continue; }
            // Compute the disk location of this record for write-back.
            let rec_sec  = inner.bpb.cluster_sector(c) + (i * DIR_ENTRY_SIZE / SECTOR) as u64;
            let rec_off  = (i * DIR_ENTRY_SIZE) % SECTOR;
            out.push(DirEntry {
                name, is_dir: attr & ATTR_DIRECTORY != 0,
                cluster, size, rec_sec, rec_off,
            });
        }
    }
    Ok(out)
}

/// Decode a reconstructed LFN unit buffer (`lfn[0..lfn_max]`) into a lowercase
/// `String`. Stops at the first `0x0000` terminator (the rest is `0xFFFF`
/// padding). ASCII units map straight to a `char`; any non-ASCII unit is decoded
/// via `char::from_u32` (replacement char on failure — our authored names are
/// ASCII, this is just robustness). Lowercased to match the lookup key.
fn decode_long_name(units: &[u16]) -> String {
    let mut out = String::new();
    for &u in units {
        if u == 0x0000 { break; }
        let ch = if u < 0x80 {
            (u as u8 as char)
        } else {
            char::from_u32(u as u32).unwrap_or('\u{FFFD}')
        };
        for lc in ch.to_lowercase() { out.push(lc); }
    }
    out
}

/// LFN checksum of an 11-byte 8.3 short name (FAT spec `ChkSum`). Mirrors
/// [`FatWriter::lfn_checksum`] for the read/validate path.
fn lfn_checksum_short(short: &[u8; 11]) -> u8 {
    let mut sum: u8 = 0;
    for &c in short {
        sum = ((sum & 1) << 7).wrapping_add(sum >> 1).wrapping_add(c);
    }
    sum
}

/// Convert a raw 8.3 entry name (11 bytes) into "name.ext" lowercase.
fn decode_short_name(raw: &[u8]) -> String {
    if raw.len() < 11 { return String::new(); }
    let base: &[u8] = trim_trailing_spaces(&raw[0..8]);
    let ext:  &[u8] = trim_trailing_spaces(&raw[8..11]);
    let mut out = String::new();
    for &b in base {
        let c = if b == 0x05 { 0xE5u8 } else { b };
        out.push(c.to_ascii_lowercase() as char);
    }
    if !ext.is_empty() {
        out.push('.');
        for &b in ext { out.push(b.to_ascii_lowercase() as char); }
    }
    out
}

fn trim_trailing_spaces(s: &[u8]) -> &[u8] {
    let mut e = s.len();
    while e > 0 && (s[e-1] == b' ' || s[e-1] == 0) { e -= 1; }
    &s[..e]
}

/// Encode "name.ext" → 11-byte 8.3 short entry. Truncates/upper-cases.
fn encode_short_name(name: &str) -> [u8; 11] {
    let mut out = [b' '; 11];
    let upper = name.to_ascii_uppercase();
    let (base, ext) = match upper.rsplit_once('.') {
        Some((b, e)) => (b, e),
        None         => (upper.as_str(), ""),
    };
    for (i, b) in base.bytes().take(8).enumerate() { out[i] = b; }
    for (i, b) in ext.bytes().take(3).enumerate()  { out[8 + i] = b; }
    out
}

/// The FAT32 filesystem instance. Wraps a shared `Inner` behind a Mutex.
pub struct Fat32Fs {
    inner: Arc<Mutex<Inner>>,
}

impl Fat32Fs {
    /// Parse the BPB off `dev` (sector 0) and return a mounted FS over any
    /// block device (an AHCI port, a partition, …).
    pub fn from_blockdev(mut dev: alloc::boxed::Box<dyn crate::blockdev::BlockDevice + Send>) -> Result<Self, VfsError> {
        let mut sec0 = [0u8; SECTOR];
        dev.read_blocks(0, &mut sec0).map_err(map_block_err)?;
        let bpb = Bpb::parse(&sec0)?;
        crate::binfo!(
            "fat32",
            "BPB ok rsvd={} fats={} fatsz={} clus_sec={} root_cluster={} tot_sec={}",
            bpb.rsvd_sec_cnt, bpb.num_fats, bpb.fat_sz32, bpb.sec_per_cluster,
            bpb.root_clus, bpb.tot_sec32,
        );
        Ok(Self { inner: Arc::new(Mutex::new(Inner {
            dev,
            bpb,
        })) })
    }

    /// Take the AHCI port from `PORT0`, parse the BPB, return a mounted FS.
    pub fn from_ahci_port(port: AhciPort) -> Result<Self, VfsError> {
        Self::from_blockdev(alloc::boxed::Box::new(port))
    }

    /// True while an open file still holds a clone of the backing `Inner`.
    /// The mount itself holds one `Arc` ref; each open `Fat32File` holds one
    /// more (`Arc::clone(&self.inner)` in `open`). So `strong_count > 1` means
    /// the device is physically live behind an open fd — unmounting now would
    /// let a later `acquire_port`/`bringup` reprogram this port's PxCLB/PxFB
    /// out from under that live file (DMA corruption). `unmount` consults this.
    pub fn is_busy(&self) -> bool {
        alloc::sync::Arc::strong_count(&self.inner) > 1
    }

    /// Look up `path` (split components) walking from the root cluster.
    /// Returns the DirEntry of the leaf. Path is interpreted case-insensitive.
    fn lookup(&self, parts: &[&str]) -> Result<DirEntry, VfsError> {
        let mut inner = self.inner.lock();
        let mut cur_cluster = inner.bpb.root_clus;
        let mut last: Option<DirEntry> = None;
        for (i, p) in parts.iter().enumerate() {
            let entries = read_dir_entries(&mut inner, cur_cluster)?;
            let want = p.to_ascii_lowercase();
            let found = entries.into_iter().find(|e| e.name == want)
                .ok_or(VfsError::NotFound)?;
            let last_part = i == parts.len() - 1;
            if !last_part {
                if !found.is_dir { return Err(VfsError::NotDirectory); }
                cur_cluster = found.cluster;
            }
            last = Some(found);
        }
        last.ok_or(VfsError::NotFound)
    }

    fn lookup_parent_and_name<'a>(&self, parts: &'a [&str])
        -> Result<(u32, &'a str), VfsError>
    {
        if parts.is_empty() { return Err(VfsError::InvalidPath); }
        let mut inner = self.inner.lock();
        let mut cur_cluster = inner.bpb.root_clus;
        if parts.len() == 1 {
            return Ok((cur_cluster, parts[0]));
        }
        for p in &parts[..parts.len()-1] {
            let entries = read_dir_entries(&mut inner, cur_cluster)?;
            let want = p.to_ascii_lowercase();
            let found = entries.into_iter().find(|e| e.name == want)
                .ok_or(VfsError::NotFound)?;
            if !found.is_dir { return Err(VfsError::NotDirectory); }
            cur_cluster = found.cluster;
        }
        Ok((cur_cluster, parts[parts.len()-1]))
    }

    /// Create a regular file in `parent_cluster` with given short name,
    /// allocating its first cluster. Returns the new DirEntry.
    fn create_file(&self, parent_cluster: u32, name: &str) -> Result<DirEntry, VfsError> {
        let mut inner = self.inner.lock();
        // Refuse duplicate.
        let existing = read_dir_entries(&mut inner, parent_cluster)?;
        let want = name.to_ascii_lowercase();
        if existing.iter().any(|e| e.name == want) {
            return Err(VfsError::AlreadyExists);
        }

        // Allocate the first data cluster (so writes have somewhere to go).
        let first_cluster = inner.alloc_cluster()?;

        // Build the 32-byte short directory record.
        let raw_name = encode_short_name(name);
        let mut rec = [0u8; DIR_ENTRY_SIZE];
        rec[0..11].copy_from_slice(&raw_name);
        rec[11] = ATTR_ARCHIVE;
        rec[20..22].copy_from_slice(&((first_cluster >> 16) as u16).to_le_bytes());
        rec[26..28].copy_from_slice(&(first_cluster as u16).to_le_bytes());
        // size = 0 initially (rec[28..32] left zero).

        // Find a free slot in the parent directory, extending its cluster chain
        // if every existing cluster is full.
        let (rec_sec, rec_off) = inner.add_dir_record(parent_cluster, &rec)?;
        Ok(DirEntry {
            name: want, is_dir: false, cluster: first_cluster,
            size: 0, rec_sec, rec_off,
        })
    }

    /// Update the on-disk size + first-cluster fields of an entry.
    fn update_entry(&self, entry: &DirEntry) -> Result<(), VfsError> {
        let mut inner = self.inner.lock();
        let mut sec_buf = [0u8; SECTOR];
        inner.read_sector(entry.rec_sec, &mut sec_buf)?;
        let rec = &mut sec_buf[entry.rec_off..entry.rec_off + DIR_ENTRY_SIZE];
        rec[20..22].copy_from_slice(&((entry.cluster >> 16) as u16).to_le_bytes());
        rec[26..28].copy_from_slice(&(entry.cluster as u16).to_le_bytes());
        rec[28..32].copy_from_slice(&entry.size.to_le_bytes());
        inner.write_sector(entry.rec_sec, &sec_buf)?;
        Ok(())
    }
}

impl FileSystem for Fat32Fs {
    async fn open(&self, path: &[&str], flags: OpenFlags) -> Result<FileImpl, VfsError> {
        let entry = match self.lookup(path) {
            Ok(e) => e,
            Err(VfsError::NotFound) if flags.contains(OpenFlags::CREATE) => {
                let (parent_cluster, name) = self.lookup_parent_and_name(path)?;
                self.create_file(parent_cluster, name)?
            }
            Err(e) => return Err(e),
        };
        if entry.is_dir { return Err(VfsError::IsDirectory); }
        let file = Fat32File {
            fs:       Arc::clone(&self.inner),
            entry:    entry.clone(),
            cached:   None,
            pos:      0,
            truncate: flags.contains(OpenFlags::TRUNCATE),
        };
        Ok(FileImpl::Fat32(file))
    }

    async fn create(&self, path: &[&str]) -> Result<(), VfsError> {
        let (parent_cluster, name) = self.lookup_parent_and_name(path)?;
        self.create_file(parent_cluster, name).map(|_| ())
    }

    async fn unlink(&self, _path: &[&str]) -> Result<(), VfsError> { Err(VfsError::Unsupported) }

    async fn mkdir(&self, path: &[&str]) -> Result<(), VfsError> {
        // Resolve the parent cluster first; `lookup_parent_and_name` takes the
        // inner lock and releases it, so we can re-lock below without deadlock
        // (same ordering as `create` → `create_file`).
        let (parent_cluster, name) = self.lookup_parent_and_name(path)?;
        let mut inner = self.inner.lock();

        // Reject a duplicate name in the parent.
        let existing = read_dir_entries(&mut inner, parent_cluster)?;
        let want = name.to_ascii_lowercase();
        if existing.iter().any(|e| e.name == want) {
            return Err(VfsError::AlreadyExists);
        }

        // Allocate the new directory's first cluster (zeroed + EOC).
        let new_cluster = inner.alloc_cluster()?;

        // The ".." cluster is 0 when the parent is the root directory (FAT spec).
        let dotdot_cluster = if parent_cluster == inner.bpb.root_clus { 0 } else { parent_cluster };

        // Build the "." and ".." records as the first two slots of the new dir.
        let mut cbuf = alloc::vec![0u8; inner.bpb.cluster_bytes()];
        // "."  -> self
        cbuf[0..11].copy_from_slice(b".          ");
        cbuf[11] = ATTR_DIRECTORY;
        cbuf[20..22].copy_from_slice(&((new_cluster >> 16) as u16).to_le_bytes());
        cbuf[26..28].copy_from_slice(&(new_cluster as u16).to_le_bytes());
        // ".." -> parent (0 at root)
        cbuf[32..43].copy_from_slice(b"..         ");
        cbuf[43] = ATTR_DIRECTORY;
        cbuf[52..54].copy_from_slice(&((dotdot_cluster >> 16) as u16).to_le_bytes());
        cbuf[58..60].copy_from_slice(&(dotdot_cluster as u16).to_le_bytes());
        // remaining slots stay zero (0x00 terminates the dir scan).
        inner.write_cluster(new_cluster, &cbuf)?;

        // Add the directory record for `name` to the parent (extends the parent
        // chain if it is full).
        let raw = encode_short_name(name);
        let mut rec = [0u8; DIR_ENTRY_SIZE];
        rec[0..11].copy_from_slice(&raw);
        rec[11] = ATTR_DIRECTORY;
        rec[20..22].copy_from_slice(&((new_cluster >> 16) as u16).to_le_bytes());
        rec[26..28].copy_from_slice(&(new_cluster as u16).to_le_bytes());
        // size = 0 for a directory (rec[28..32] left zero).
        inner.add_dir_record(parent_cluster, &rec)?;
        Ok(())
    }

    async fn rmdir(&self, _path: &[&str]) -> Result<(), VfsError>  { Err(VfsError::Unsupported) }
    async fn rename(&self, _src: &[&str], _dst: &[&str]) -> Result<(), VfsError> { Err(VfsError::Unsupported) }

    async fn readdir(&self, path: &[&str]) -> Result<Vec<VfsDirent>, VfsError> {
        let mut inner = self.inner.lock();
        let cluster = if path.is_empty() {
            inner.bpb.root_clus
        } else {
            drop(inner);
            let e = self.lookup(path)?;
            if !e.is_dir { return Err(VfsError::NotDirectory); }
            inner = self.inner.lock();
            e.cluster
        };
        let entries = read_dir_entries(&mut inner, cluster)?;
        Ok(entries.into_iter().map(|e| VfsDirent {
            name: e.name,
            kind: if e.is_dir { VfsKind::Dir } else { VfsKind::Reg },
        }).collect())
    }

    async fn stat(&self, path: &[&str]) -> Result<VfsStat, VfsError> {
        if path.is_empty() {
            return Ok(VfsStat { kind: VfsKind::Dir, size: 0 });
        }
        let e = self.lookup(path)?;
        Ok(VfsStat {
            kind: if e.is_dir { VfsKind::Dir } else { VfsKind::Reg },
            size: e.size as u64,
        })
    }
}

/// One open file. Caches the cluster chain on demand for read; rebuilds it on
/// write extension.
pub struct Fat32File {
    fs:       Arc<Mutex<Inner>>,
    entry:    DirEntry,
    cached:   Option<Vec<u32>>,
    pos:      u64,
    truncate: bool,
}

impl Fat32File {
    fn ensure_chain(&mut self) -> Result<&Vec<u32>, VfsError> {
        if self.cached.is_none() {
            let mut inner = self.fs.lock();
            let chain = inner.chain(self.entry.cluster)?;
            self.cached = Some(chain);
        }
        Ok(self.cached.as_ref().unwrap())
    }

    fn invalidate_chain(&mut self) { self.cached = None; }
}

impl File for Fat32File {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, VfsError> {
        if self.pos >= self.entry.size as u64 || buf.is_empty() { return Ok(0); }
        let chain = self.ensure_chain()?.clone();
        let mut inner = self.fs.lock();
        let cluster_bytes = inner.bpb.cluster_bytes() as u64;
        let cluster_idx   = (self.pos / cluster_bytes) as usize;
        let within        = (self.pos % cluster_bytes) as usize;
        if cluster_idx >= chain.len() { return Ok(0); }
        let cluster = chain[cluster_idx];
        let mut cbuf = alloc::vec![0u8; cluster_bytes as usize];
        inner.read_cluster(cluster, &mut cbuf)?;
        let avail_in_cluster = (cluster_bytes as usize).saturating_sub(within);
        let avail_in_file    = (self.entry.size as u64 - self.pos) as usize;
        let n = buf.len().min(avail_in_cluster).min(avail_in_file);
        buf[..n].copy_from_slice(&cbuf[within..within + n]);
        self.pos += n as u64;
        Ok(n)
    }

    async fn write(&mut self, buf: &[u8]) -> Result<usize, VfsError> {
        if buf.is_empty() { return Ok(0); }
        if self.truncate {
            // For MVP: truncate semantics = reset cluster's content + file size.
            // We don't free extra clusters in the chain; on a fresh-create the
            // chain is one cluster and that's exactly what we wanted anyway.
            self.entry.size = 0;
            self.pos = 0;
            self.truncate = false;
            self.invalidate_chain();
        }
        let chain = self.ensure_chain()?.clone();
        let cluster_bytes = {
            let inner = self.fs.lock();
            inner.bpb.cluster_bytes() as u64
        };
        // Extend chain if needed.
        let needed_clusters = ((self.pos + buf.len() as u64 + cluster_bytes - 1) / cluster_bytes) as usize;
        let chain = if needed_clusters > chain.len() {
            let extra = needed_clusters - chain.len();
            let mut inner = self.fs.lock();
            let mut chain = chain;
            for _ in 0..extra {
                let new_c = inner.alloc_cluster()?;
                // Link previous tail → new_c.
                if let Some(&last) = chain.last() {
                    inner.write_fat_entry(last, new_c)?;
                }
                chain.push(new_c);
            }
            drop(inner);
            // Persist the new chain in cache and possibly update entry's first cluster.
            self.cached = Some(chain.clone());
            chain
        } else {
            chain
        };

        let cluster_idx = (self.pos / cluster_bytes) as usize;
        let within      = (self.pos % cluster_bytes) as usize;
        let cluster     = chain[cluster_idx];
        // Read-modify-write that cluster.
        let mut inner = self.fs.lock();
        let mut cbuf = alloc::vec![0u8; cluster_bytes as usize];
        inner.read_cluster(cluster, &mut cbuf)?;
        let n = buf.len().min(cluster_bytes as usize - within);
        cbuf[within..within + n].copy_from_slice(&buf[..n]);
        inner.write_cluster(cluster, &cbuf)?;
        drop(inner);

        self.pos += n as u64;
        if self.pos > self.entry.size as u64 {
            self.entry.size = self.pos as u32;
        }
        // Update the dir entry: first_cluster (chain may now start at a new
        // cluster if file was empty before) + size.
        self.entry.cluster = chain[0];
        update_entry_on_disk(&self.fs, &self.entry)?;
        Ok(n)
    }

    async fn seek(&mut self, off: i64, whence: Whence) -> Result<u64, VfsError> {
        let base: i64 = match whence {
            Whence::Set => 0,
            Whence::Cur => self.pos as i64,
            Whence::End => self.entry.size as i64,
        };
        let new = base.saturating_add(off);
        if new < 0 { return Err(VfsError::InvalidPath); }
        self.pos = new as u64;
        Ok(self.pos)
    }

    async fn stat(&self) -> Result<VfsStat, VfsError> {
        Ok(VfsStat { kind: VfsKind::Reg, size: self.entry.size as u64 })
    }
}

/// Free-standing version of `Fat32Fs::update_entry` callable from
/// `Fat32File::write` without needing a wrapper Arc clone.
fn update_entry_on_disk(fs: &Arc<Mutex<Inner>>, entry: &DirEntry) -> Result<(), VfsError> {
    let mut inner = fs.lock();
    let mut sec_buf = [0u8; SECTOR];
    inner.read_sector(entry.rec_sec, &mut sec_buf)?;
    let rec = &mut sec_buf[entry.rec_off..entry.rec_off + DIR_ENTRY_SIZE];
    rec[20..22].copy_from_slice(&((entry.cluster >> 16) as u16).to_le_bytes());
    rec[26..28].copy_from_slice(&(entry.cluster as u16).to_le_bytes());
    rec[28..32].copy_from_slice(&entry.size.to_le_bytes());
    inner.write_sector(entry.rec_sec, &sec_buf)?;
    Ok(())
}

/// Format `dev` as a fresh FAT32 volume (`mkfs.fat32`).
///
/// Operates purely through `dev.write_blocks`; no mount-table involvement.
/// Geometry follows the canonical fatgen103 / Microsoft `DskTableFAT32` math
/// (same as `mkfs.fat`) so the result passes `fsck.fat` and is mountable by
/// `mtools` and our own reader. Layout written:
///   LBA 0      boot sector (BPB)
///   LBA 1      FSInfo
///   LBA 6      backup boot sector (+ LBA 7 backup FSInfo)
///   reserved.. FAT #0 then FAT #1, each `fat_sz32` sectors
///   data_start root directory cluster (cluster 2), zeroed
///
/// All multi-byte fields are little-endian. Any write error maps to
/// [`VfsError::IoError`].
pub fn format(dev: &mut dyn BlockDevice) -> Result<(), VfsError> {
    if dev.block_size() != SECTOR as u32 {
        return Err(VfsError::IoError);
    }

    // Total sectors, clamped to u32 (our partitions are < 2 TiB so this is
    // exact; cap defensively rather than truncate-wrap on a hypothetical
    // monster device).
    let tot_sec: u32 = core::cmp::min(dev.block_count(), u32::MAX as u64) as u32;

    const RESERVED_SEC_CNT: u32 = 32;
    const NUM_FATS: u32 = 2;
    const ROOT_CLUS: u32 = 2;

    // sec_per_clus from the Microsoft DskTableFAT32 (units = 512-byte sectors).
    let sec_per_clus: u32 = if tot_sec <= 66_600 {
        // Too small to be valid FAT32 — our partitions are large.
        return Err(VfsError::IoError);
    } else if tot_sec <= 532_480 {
        1
    } else if tot_sec <= 16_777_216 {
        8
    } else if tot_sec <= 33_554_432 {
        16
    } else if tot_sec <= 67_108_864 {
        32
    } else {
        64
    };

    // fatgen103 FATSz (FAT32, RootDirSectors = 0). Ceil-divide; a slight
    // over-allocation of the FAT is correct and what mkfs.fat does too.
    // tmp2 = (256 * sec_per_clus + num_fats) / 2  fits comfortably in u32.
    let tmp1: u32 = tot_sec.saturating_sub(RESERVED_SEC_CNT);
    let tmp2: u32 = (256 * sec_per_clus + NUM_FATS) / 2;
    // Ceil-divide in u64: at the u32 clamp (≈2 TiB, spc=64) `tmp1 + (tmp2-1)`
    // would overflow u32 and wrap fat_sz32 to 0 → a corrupt, self-unmountable
    // FAT. The quotient still fits u32 (fat_sz32 ≤ tot_sec).
    let fat_sz32: u32 = ((tmp1 as u64 + (tmp2 as u64 - 1)) / tmp2 as u64) as u32;

    // Data region geometry + the FAT32 minimum-cluster sanity check.
    let data_start: u32 = RESERVED_SEC_CNT + NUM_FATS * fat_sz32;
    if tot_sec <= data_start {
        return Err(VfsError::IoError);
    }
    let clusters: u32 = (tot_sec - data_start) / sec_per_clus;
    if clusters < 65_525 {
        // Below the FAT32 cluster minimum — not a valid FAT32 volume.
        return Err(VfsError::IoError);
    }

    // --- Boot sector (LBA 0) ---------------------------------------------
    let mut boot = [0u8; SECTOR];
    boot[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);     // jmp short + nop
    boot[3..11].copy_from_slice(b"MSWIN4.1");            // OEM name
    boot[11..13].copy_from_slice(&(SECTOR as u16).to_le_bytes()); // bytes/sec
    boot[13] = sec_per_clus as u8;
    boot[14..16].copy_from_slice(&(RESERVED_SEC_CNT as u16).to_le_bytes());
    boot[16] = NUM_FATS as u8;
    boot[17..19].copy_from_slice(&0u16.to_le_bytes());   // root_ent_cnt = 0
    boot[19..21].copy_from_slice(&0u16.to_le_bytes());   // tot_sec16 = 0
    boot[21] = 0xF8;                                      // media (fixed disk)
    boot[22..24].copy_from_slice(&0u16.to_le_bytes());   // fat_sz16 = 0
    boot[24..26].copy_from_slice(&0x3Fu16.to_le_bytes()); // sec/track (cosmetic)
    boot[26..28].copy_from_slice(&0xFFu16.to_le_bytes()); // num_heads (cosmetic)
    boot[28..32].copy_from_slice(&0u32.to_le_bytes());   // hidden_sec = 0
    boot[32..36].copy_from_slice(&tot_sec.to_le_bytes());
    boot[36..40].copy_from_slice(&fat_sz32.to_le_bytes());
    boot[40..42].copy_from_slice(&0u16.to_le_bytes());   // ext_flags (mirrored)
    boot[42..44].copy_from_slice(&0u16.to_le_bytes());   // fs_ver = 0.0
    boot[44..48].copy_from_slice(&ROOT_CLUS.to_le_bytes());
    boot[48..50].copy_from_slice(&1u16.to_le_bytes());   // fsinfo sector
    boot[50..52].copy_from_slice(&6u16.to_le_bytes());   // backup boot sector
    // boot[52..64] reserved = 0
    boot[64] = 0x80;                                     // drive number
    boot[65] = 0;                                        // reserved
    boot[66] = 0x29;                                     // ext boot signature
    let mut vol_id = [0u8; 4];
    crate::rng::fill(&mut vol_id);
    boot[67..71].copy_from_slice(&vol_id);               // volume serial
    boot[71..82].copy_from_slice(b"RUOS       ");        // volume label (11)
    boot[82..90].copy_from_slice(b"FAT32   ");           // fs type (8)
    boot[510] = 0x55;
    boot[511] = 0xAA;

    // --- FSInfo (LBA 1) ---------------------------------------------------
    let mut fsinfo = [0u8; SECTOR];
    fsinfo[0..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());   // lead sig "RRaA"
    // [4..484] reserved = 0
    fsinfo[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes()); // struct sig "rrAa"
    // Free count = 0xFFFFFFFF ("unknown"). The volume is mutated after format
    // (mkdir during author, file copies later) and the shared alloc_cluster does
    // not maintain FSInfo, so a fixed count goes stale and fsck flags "Free
    // cluster summary wrong". The spec sentinel tells the OS to recompute; fsck
    // then skips the check. next_free stays a (harmless) search hint.
    fsinfo[488..492].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    fsinfo[492..496].copy_from_slice(&3u32.to_le_bytes());          // next free hint
    // [496..508] reserved = 0
    fsinfo[508..512].copy_from_slice(&0xAA55_0000u32.to_le_bytes()); // trail sig

    // Helper: map a write error to VfsError::IoError.
    let one = |dev: &mut dyn BlockDevice, lba: u64, buf: &[u8]| -> Result<(), VfsError> {
        dev.write_blocks(lba, buf).map_err(map_block_err)
    };

    one(dev, 0, &boot)?;
    one(dev, 1, &fsinfo)?;
    // Backup boot region at sector 6 (+ backup FSInfo at 7 for completeness).
    one(dev, 6, &boot)?;
    one(dev, 7, &fsinfo)?;

    // --- Zero both FATs ---------------------------------------------------
    let zero = [0u8; SECTOR];
    for fat_idx in 0..NUM_FATS {
        let fat_start = u64::from(RESERVED_SEC_CNT) + u64::from(fat_idx) * u64::from(fat_sz32);
        for s in 0..u64::from(fat_sz32) {
            one(dev, fat_start + s, &zero)?;
        }
    }

    // Seed the first 3 entries in each FAT:
    //   FAT[0] = 0x0FFFFFF8 (media byte in low 8 bits | EOC bits)
    //   FAT[1] = 0x0FFFFFFF (clean/no-error flags)
    //   FAT[2] = 0x0FFFFFFF (root cluster = end of chain)
    let mut fat0 = [0u8; SECTOR];
    fat0[0..4].copy_from_slice(&0x0FFF_FFF8u32.to_le_bytes());
    fat0[4..8].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
    fat0[8..12].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
    for fat_idx in 0..NUM_FATS {
        let fat_start = u64::from(RESERVED_SEC_CNT) + u64::from(fat_idx) * u64::from(fat_sz32);
        one(dev, fat_start, &fat0)?;
    }

    // --- Root directory cluster (cluster 2) -------------------------------
    // Zero every sector, then drop a volume-label entry as the first record so
    // the boot-sector label is mirrored in the root dir (what mkfs.fat does;
    // without it fsck.fat warns + "auto-removes" the label). The label entry
    // is an ATTR_VOLUME_ID record whose 11 name bytes hold the space-padded
    // label; our reader skips it (ATTR_VOLUME_ID) so it stays invisible.
    let root_sec = u64::from(data_start); // (2 - 2) * sec_per_clus == 0
    let mut root0 = [0u8; SECTOR];
    root0[0..11].copy_from_slice(b"RUOS       "); // 8.3 name field = label
    root0[11] = ATTR_VOLUME_ID;                   // attr
    // remaining fields (times, clusters, size) stay zero
    one(dev, root_sec, &root0)?;
    for s in 1..u64::from(sec_per_clus) {
        one(dev, root_sec + s, &zero)?;
    }

    crate::binfo!(
        "fat32",
        "format ok tot_sec={} spc={} fat_sz32={} clusters={} data_start={}",
        tot_sec, sec_per_clus, fat_sz32, clusters, data_start,
    );
    Ok(())
}

/// A synchronous, borrow-based FAT32 writer for the disk-authoring path.
///
/// Unlike [`Fat32Fs`] (async, behind `Arc<Mutex<Inner>>`, tied to the global
/// `/mnt` mount table), this operates directly on a borrowed `&mut dyn
/// BlockDevice` with no allocation of a long-lived FS object and no async. It
/// reads the BPB once (sector 0) and exposes exactly the cluster/dir primitives
/// `create_dirs` needs. The cluster/dir logic mirrors [`Inner`]'s
/// (`alloc_cluster`, `add_dir_record`, FAT-entry mirroring) so a volume it
/// authors is identical to what the mounted driver would produce.
pub struct FatWriter<'a> {
    dev: &'a mut dyn BlockDevice,
    bpb: Bpb,
    /// Lowest cluster index that *might* be free — the next-free-cluster search
    /// hint (FSInfo `Nxt_Free` in spirit, kept in RAM). All allocator paths
    /// (`alloc_cluster`, `alloc_chain`) start their scan here and bump it past
    /// what they hand out, so allocation is O(1) amortised instead of an O(n)
    /// rescan from cluster 2 each call. Seeded by one cached scan in `open`.
    next_free: u32,
}

impl<'a> FatWriter<'a> {
    /// Parse the BPB off sector 0 of a freshly-`format`ted volume.
    pub fn open(dev: &'a mut dyn BlockDevice) -> Result<Self, VfsError> {
        let mut sec0 = [0u8; SECTOR];
        dev.read_blocks(0, &mut sec0).map_err(map_block_err)?;
        let bpb = Bpb::parse(&sec0)?;
        let mut w = Self { dev, bpb, next_free: 2 };
        w.next_free = w.scan_first_free()?;
        Ok(w)
    }

    /// Scan the whole FAT **once** for the first free (`0`) cluster, reading each
    /// FAT sector a single time and walking the 128 u32 entries it holds in
    /// memory (never re-reading per entry — that is what made the old
    /// `alloc_cluster` O(n^2)). Returns the cluster index, or `max_cluster` if
    /// the volume is full (callers turn that into `NoSpace`). On a
    /// fresh-formatted ESP with a handful of dirs this lands within the first
    /// FAT sector.
    fn scan_first_free(&mut self) -> Result<u32, VfsError> {
        let max_cluster = self.bpb.max_cluster();
        if max_cluster <= 2 { return Ok(max_cluster); }
        let mut sec_buf = [0u8; SECTOR];
        let mut n = 2u32;
        let fat_base = u64::from(self.bpb.rsvd_sec_cnt);
        while n < max_cluster {
            let sec = fat_base + (n as u64 * 4) / SECTOR as u64;
            self.read_sector(sec, &mut sec_buf)?;
            // Entries from `n` up to the end of this sector.
            let first_off = ((n as u64 * 4) % SECTOR as u64) as usize;
            let mut off = first_off;
            while off + 4 <= SECTOR && n < max_cluster {
                let entry = u32::from_le_bytes([
                    sec_buf[off], sec_buf[off+1], sec_buf[off+2], sec_buf[off+3]
                ]) & 0x0FFF_FFFF;
                if entry == 0 { return Ok(n); }
                n += 1;
                off += 4;
            }
        }
        Ok(max_cluster)
    }

    fn read_sector(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), VfsError> {
        self.dev.read_blocks(lba, buf).map_err(map_block_err)
    }
    fn write_sector(&mut self, lba: u64, buf: &[u8]) -> Result<(), VfsError> {
        self.dev.write_blocks(lba, buf).map_err(map_block_err)
    }

    fn read_cluster(&mut self, n: u32, out: &mut [u8]) -> Result<(), VfsError> {
        let sectors = u64::from(self.bpb.sec_per_cluster);
        let sec = self.bpb.cluster_sector(n);
        if out.len() < self.bpb.cluster_bytes() { return Err(VfsError::IoError); }
        for s in 0..sectors {
            let sub = &mut out[(s as usize) * SECTOR..((s+1) as usize) * SECTOR];
            self.read_sector(sec + s, sub)?;
        }
        Ok(())
    }

    fn write_cluster(&mut self, n: u32, data: &[u8]) -> Result<(), VfsError> {
        let sectors = u64::from(self.bpb.sec_per_cluster);
        let sec = self.bpb.cluster_sector(n);
        for s in 0..sectors {
            let sub = &data[(s as usize) * SECTOR..((s+1) as usize) * SECTOR];
            self.write_sector(sec + s, sub)?;
        }
        Ok(())
    }

    /// Follow the FAT chain starting at `start`, returning every cluster index.
    /// Bounded by the cluster count so a corrupt/cyclic FAT can't loop forever.
    fn chain(&mut self, start: u32) -> Result<Vec<u32>, VfsError> {
        let mut out = Vec::new();
        let mut sec_buf = [0u8; SECTOR];
        let mut cur = start;
        let mut cached_sec: Option<u64> = None;
        let max_clusters = self.bpb.max_cluster() as usize;
        while cur >= 2 && (cur & 0x0FFF_FFFF) < EOC {
            if out.len() >= max_clusters { return Err(VfsError::IoError); }
            out.push(cur);
            let (sec, off) = self.bpb.fat_entry_loc(cur);
            if cached_sec != Some(sec) {
                self.read_sector(sec, &mut sec_buf)?;
                cached_sec = Some(sec);
            }
            let entry = u32::from_le_bytes([
                sec_buf[off], sec_buf[off+1], sec_buf[off+2], sec_buf[off+3]
            ]) & 0x0FFF_FFFF;
            if entry == 0 { return Err(VfsError::IoError); }
            cur = entry;
        }
        Ok(out)
    }

    /// Write a single FAT entry (lower 28 bits), mirrored to every FAT copy.
    fn write_fat_entry(&mut self, cluster: u32, value: u32) -> Result<(), VfsError> {
        let (sec0, off) = self.bpb.fat_entry_loc(cluster);
        let mut sec_buf = [0u8; SECTOR];
        for fat_idx in 0..u64::from(self.bpb.num_fats) {
            let sec = sec0 + fat_idx * u64::from(self.bpb.fat_sz32);
            self.read_sector(sec, &mut sec_buf)?;
            let mut e = u32::from_le_bytes([
                sec_buf[off], sec_buf[off+1], sec_buf[off+2], sec_buf[off+3]
            ]);
            e = (e & 0xF000_0000) | (value & 0x0FFF_FFFF);
            sec_buf[off..off+4].copy_from_slice(&e.to_le_bytes());
            self.write_sector(sec, &sec_buf)?;
        }
        Ok(())
    }

    /// Allocate one free cluster, set it to EOC, and zero its data so no stale
    /// bytes leak. Starts the scan at the `next_free` hint (caching each FAT
    /// sector across its 128 entries) and advances the hint past the cluster it
    /// returns, so repeated allocs are O(1) amortised rather than rescanning from
    /// cluster 2. Shared by every authoring path (`mkdir`, `add_dir_run`,
    /// `create_dirs`) so two allocations can never collide on the same cluster.
    fn alloc_cluster(&mut self) -> Result<u32, VfsError> {
        let n = self.take_free_cluster()?;
        self.write_fat_entry(n, 0x0FFF_FFFF)?;
        let zero = alloc::vec![0u8; self.bpb.cluster_bytes()];
        self.write_cluster(n, &zero)?;
        Ok(n)
    }

    /// Find the next free cluster at/after `next_free`, advance the hint past it,
    /// and return it. Does NOT touch the FAT entry or the data (callers do that).
    /// Reads each FAT sector once and walks its 128 entries in memory.
    fn take_free_cluster(&mut self) -> Result<u32, VfsError> {
        let max_cluster = self.bpb.max_cluster();
        let mut sec_buf = [0u8; SECTOR];
        let mut cached_sec: Option<u64> = None;
        let fat_base = u64::from(self.bpb.rsvd_sec_cnt);
        let mut n = self.next_free.max(2);
        while n < max_cluster {
            let sec = fat_base + (n as u64 * 4) / SECTOR as u64;
            if cached_sec != Some(sec) {
                self.read_sector(sec, &mut sec_buf)?;
                cached_sec = Some(sec);
            }
            let off = ((n as u64 * 4) % SECTOR as u64) as usize;
            let entry = u32::from_le_bytes([
                sec_buf[off], sec_buf[off+1], sec_buf[off+2], sec_buf[off+3]
            ]) & 0x0FFF_FFFF;
            if entry == 0 {
                self.next_free = n + 1;
                return Ok(n);
            }
            n += 1;
        }
        Err(VfsError::NoSpace)
    }

    /// Allocate a chain of `n` clusters for a file and link them on disk
    /// (`first -> first+1 -> ... -> EOC`), returning the first cluster.
    ///
    /// Fast path (the append-only authoring case): the `n` clusters starting at
    /// the `next_free` hint are all free and contiguous, so the chain is
    /// `first..first+n`. Each cluster is still verified free as we go; if a
    /// non-free cluster is hit we fall back to scanning forward for free clusters
    /// (the resulting chain may then be non-contiguous, which `write_file`
    /// handles). The FAT links are written by building **whole FAT sectors in
    /// memory** (128 entries per 512-byte sector) and writing each affected
    /// sector ONCE per FAT copy — turning ~`n` individual read-modify-writes per
    /// FAT into ~`n/128`. Advances `next_free`. Data clusters are NOT pre-zeroed
    /// (the caller overwrites every byte and zero-pads the final cluster tail).
    fn alloc_chain(&mut self, n: u32) -> Result<u32, VfsError> {
        if n == 0 { return Err(VfsError::IoError); }
        let clusters = self.collect_free_clusters(n)?;
        self.write_chain_fat(&clusters)?;
        Ok(clusters[0])
    }

    /// Reserve `n` free clusters starting from the `next_free` hint, returning
    /// them in order. Verifies each candidate is free; the common case is the
    /// contiguous run `next_free..next_free+n`. Advances `next_free` past the
    /// highest reserved cluster. Does not write the FAT (see `write_chain_fat`).
    fn collect_free_clusters(&mut self, n: u32) -> Result<Vec<u32>, VfsError> {
        let max_cluster = self.bpb.max_cluster();
        let mut out: Vec<u32> = Vec::with_capacity(n as usize);
        let mut sec_buf = [0u8; SECTOR];
        let mut cached_sec: Option<u64> = None;
        let fat_base = u64::from(self.bpb.rsvd_sec_cnt);
        let mut cur = self.next_free.max(2);
        while (out.len() as u32) < n {
            if cur >= max_cluster { return Err(VfsError::NoSpace); }
            let sec = fat_base + (cur as u64 * 4) / SECTOR as u64;
            if cached_sec != Some(sec) {
                self.read_sector(sec, &mut sec_buf)?;
                cached_sec = Some(sec);
            }
            let off = ((cur as u64 * 4) % SECTOR as u64) as usize;
            let entry = u32::from_le_bytes([
                sec_buf[off], sec_buf[off+1], sec_buf[off+2], sec_buf[off+3]
            ]) & 0x0FFF_FFFF;
            if entry == 0 {
                out.push(cur);
            }
            cur += 1;
        }
        // Advance the hint past every cluster we examined. In the contiguous fast
        // path this equals `out.last()+1`; after a scattered fallback it also
        // skips the interleaved non-free clusters we already rejected.
        self.next_free = cur;
        Ok(out)
    }

    /// Write the FAT chain for an ordered cluster list: `clusters[i]` points to
    /// `clusters[i+1]`, and the last points to EOC. Mirrors to every FAT copy.
    /// Builds whole FAT sectors in memory and writes each affected sector once
    /// per FAT, so a long chain costs ~`ceil(span/128) * num_fats` sector writes
    /// instead of one read-modify-write per cluster.
    fn write_chain_fat(&mut self, clusters: &[u32]) -> Result<(), VfsError> {
        if clusters.is_empty() { return Ok(()); }
        let fat_sz = u64::from(self.bpb.fat_sz32);
        let fat_base = u64::from(self.bpb.rsvd_sec_cnt);

        // Walk the list in index order, keeping one FAT sector resident in
        // `sec_buf`. Up to 128 consecutive entries share a sector (the
        // contiguous fast path), so we only reload — and flush, mirrored to
        // every FAT copy — when an entry crosses into a different FAT sector.
        // The list is ascending in the fast path but may have gaps after a
        // scattered fallback; this handles both.
        let mut sec_buf = [0u8; SECTOR];
        let mut loaded_sec: Option<u64> = None; // which logical FAT sector is in sec_buf
        let mut dirty = false;

        let mut i = 0usize;
        while i < clusters.len() {
            let c = clusters[i];
            let logical_sec = (c as u64 * 4) / SECTOR as u64; // relative to FAT start
            if loaded_sec != Some(logical_sec) {
                // Flush the previously loaded sector before loading a new one.
                // `dirty` is only false on the very first iteration (nothing
                // loaded yet); every reload after that follows entry writes into
                // the resident buffer, so it is always dirty here.
                if dirty {
                    let prev = loaded_sec.unwrap();
                    for fat_idx in 0..u64::from(self.bpb.num_fats) {
                        let lba = fat_base + fat_idx * fat_sz + prev;
                        self.write_sector(lba, &sec_buf)?;
                    }
                }
                // Load the new sector from FAT #0 (so we preserve the top 4 bits
                // and any neighbouring entries we are not changing).
                let lba0 = fat_base + logical_sec;
                self.read_sector(lba0, &mut sec_buf)?;
                loaded_sec = Some(logical_sec);
            }
            // Set this cluster's entry to point at the next (or EOC at the tail).
            let off = ((c as u64 * 4) % SECTOR as u64) as usize;
            let value: u32 = if i + 1 < clusters.len() {
                clusters[i + 1] & 0x0FFF_FFFF
            } else {
                0x0FFF_FFFF
            };
            let mut e = u32::from_le_bytes([
                sec_buf[off], sec_buf[off+1], sec_buf[off+2], sec_buf[off+3]
            ]);
            e = (e & 0xF000_0000) | value;
            sec_buf[off..off+4].copy_from_slice(&e.to_le_bytes());
            dirty = true;
            i += 1;
        }
        // Final flush.
        if dirty {
            let last = loaded_sec.unwrap();
            for fat_idx in 0..u64::from(self.bpb.num_fats) {
                let lba = fat_base + fat_idx * fat_sz + last;
                self.write_sector(lba, &sec_buf)?;
            }
        }
        Ok(())
    }

    /// Stream `bytes` into the data region of the file's `clusters` chain.
    ///
    /// Splits the chain into maximal **physically-contiguous** sub-runs and
    /// writes each as a few large `write_blocks` calls (capped at
    /// `MAX_WRITE_SECTORS = 8192` sectors / 4 MiB per call — the AHCI PRDT-entry
    /// limit) straight from a slice of `bytes`, with no per-cluster copy. The
    /// fully-present, sector-aligned prefix of a sub-run is written directly; any
    /// trailing partial sector and the zero tail of the file's LAST cluster are
    /// written from one small zeroed scratch buffer (so stale bytes never leak
    /// past EOF). A scattered fallback chain degrades to length-1 sub-runs (i.e.
    /// per-cluster), staying correct.
    fn write_data_run(&mut self, clusters: &[u32], bytes: &[u8]) -> Result<(), VfsError> {
        const MAX_WRITE_SECTORS: u64 = 8192;
        let cluster_bytes = self.bpb.cluster_bytes();
        let spc = u64::from(self.bpb.sec_per_cluster);

        let mut idx = 0usize; // chain index of the current sub-run start
        while idx < clusters.len() {
            // Extend a contiguous sub-run [idx, end).
            let mut end = idx + 1;
            while end < clusters.len() && clusters[end] == clusters[end - 1] + 1 {
                end += 1;
            }
            let run_len = end - idx;
            let start_lba = self.bpb.cluster_sector(clusters[idx]);
            let run_sectors = run_len as u64 * spc;

            // Byte window this sub-run covers.
            let byte_base = idx * cluster_bytes;
            let avail = bytes.len().saturating_sub(byte_base).min(run_len * cluster_bytes);
            let full_sectors = (avail / SECTOR) as u64; // sectors fully covered by `bytes`

            // 1. Write the fully-present sectors directly from `bytes`, chunked.
            let mut written: u64 = 0;
            while written < full_sectors {
                let chunk = (full_sectors - written).min(MAX_WRITE_SECTORS);
                let bstart = byte_base + (written as usize) * SECTOR;
                let blen = (chunk as usize) * SECTOR;
                self.write_sector_run(start_lba + written, &bytes[bstart..bstart + blen])?;
                written += chunk;
            }

            // 2. Tail: the partial final sector (residual bytes) plus any whole
            //    zero sectors left in this sub-run (only the file's last cluster
            //    can have these). One zeroed scratch covers both → no stale leak.
            let tail_sectors = run_sectors - full_sectors;
            if tail_sectors > 0 {
                let mut scratch = alloc::vec![0u8; (tail_sectors as usize) * SECTOR];
                let residual = avail - (full_sectors as usize) * SECTOR; // 0..512
                if residual > 0 {
                    let bstart = byte_base + (full_sectors as usize) * SECTOR;
                    scratch[..residual].copy_from_slice(&bytes[bstart..bstart + residual]);
                }
                self.write_sector_run(start_lba + full_sectors, &scratch)?;
            }

            idx = end;
        }
        Ok(())
    }

    /// Write a multi-sector buffer (already a multiple of `SECTOR`) at `lba`.
    /// Thin wrapper over the block device for the batched data path.
    fn write_sector_run(&mut self, lba: u64, buf: &[u8]) -> Result<(), VfsError> {
        self.dev.write_blocks(lba, buf).map_err(map_block_err)
    }

    /// Find a free slot in directory `dir_cluster`'s chain and write `rec`,
    /// extending the chain with a fresh cluster if every slot is taken. Mirrors
    /// `Inner::add_dir_record`.
    fn add_dir_record(&mut self, dir_cluster: u32, rec: &[u8; DIR_ENTRY_SIZE])
        -> Result<(), VfsError>
    {
        let chain = self.chain(dir_cluster)?;
        let cluster_bytes = self.bpb.cluster_bytes();
        let entries_per_cluster = cluster_bytes / DIR_ENTRY_SIZE;
        let mut buf = alloc::vec![0u8; cluster_bytes];
        for &c in &chain {
            self.read_cluster(c, &mut buf)?;
            for i in 0..entries_per_cluster {
                let first = buf[i * DIR_ENTRY_SIZE];
                if first == 0x00 || first == 0xE5 {
                    buf[i * DIR_ENTRY_SIZE..(i+1) * DIR_ENTRY_SIZE].copy_from_slice(rec);
                    self.write_cluster(c, &buf)?;
                    return Ok(());
                }
            }
        }
        // Every cluster full — link a fresh (zeroed + EOC) cluster and use slot 0.
        let last = *chain.last().ok_or(VfsError::IoError)?;
        let new_cluster = self.alloc_cluster()?;
        self.write_fat_entry(last, new_cluster)?;
        buf.iter_mut().for_each(|b| *b = 0);
        buf[0..DIR_ENTRY_SIZE].copy_from_slice(rec);
        self.write_cluster(new_cluster, &buf)?;
        Ok(())
    }

    /// Look for short-name `want` (lowercase) directly inside `dir_cluster`.
    /// Returns its first cluster if it is a directory. Mirrors the relevant bits
    /// of `read_dir_entries` without the heavy `DirEntry` decode.
    fn find_subdir(&mut self, dir_cluster: u32, want: &str) -> Result<Option<u32>, VfsError> {
        let chain = self.chain(dir_cluster)?;
        let cluster_bytes = self.bpb.cluster_bytes();
        let entries_per_cluster = cluster_bytes / DIR_ENTRY_SIZE;
        let mut buf = alloc::vec![0u8; cluster_bytes];
        for c in chain {
            self.read_cluster(c, &mut buf)?;
            for i in 0..entries_per_cluster {
                let rec = &buf[i * DIR_ENTRY_SIZE..(i+1) * DIR_ENTRY_SIZE];
                let first = rec[0];
                if first == 0x00 { return Ok(None); }     // end of directory
                if first == 0xE5 { continue; }            // deleted
                let attr = rec[11];
                if attr == ATTR_LFN { continue; }
                if attr & ATTR_VOLUME_ID != 0 { continue; }
                let name = decode_short_name(&rec[0..11]);
                if name == want {
                    if attr & ATTR_DIRECTORY == 0 { return Err(VfsError::NotDirectory); }
                    let hi = u16::from_le_bytes([rec[20], rec[21]]) as u32;
                    let lo = u16::from_le_bytes([rec[26], rec[27]]) as u32;
                    return Ok(Some((hi << 16) | lo));
                }
            }
        }
        Ok(None)
    }

    /// Create a child directory `name` under `parent_cluster`: allocate its
    /// cluster, write its `.`/`..` records (`..` cluster is 0 when the parent is
    /// the root, per the FAT spec), and add the parent's directory record.
    /// Returns the new cluster. Mirrors `Fat32Fs::mkdir` (Task 4).
    fn mkdir(&mut self, parent_cluster: u32, name: &str) -> Result<u32, VfsError> {
        let new_cluster = self.alloc_cluster()?;
        let dotdot_cluster = if parent_cluster == self.bpb.root_clus { 0 } else { parent_cluster };

        let mut cbuf = alloc::vec![0u8; self.bpb.cluster_bytes()];
        // "."  -> self
        cbuf[0..11].copy_from_slice(b".          ");
        cbuf[11] = ATTR_DIRECTORY;
        cbuf[20..22].copy_from_slice(&((new_cluster >> 16) as u16).to_le_bytes());
        cbuf[26..28].copy_from_slice(&(new_cluster as u16).to_le_bytes());
        // ".." -> parent (0 at root)
        cbuf[32..43].copy_from_slice(b"..         ");
        cbuf[43] = ATTR_DIRECTORY;
        cbuf[52..54].copy_from_slice(&((dotdot_cluster >> 16) as u16).to_le_bytes());
        cbuf[58..60].copy_from_slice(&(dotdot_cluster as u16).to_le_bytes());
        self.write_cluster(new_cluster, &cbuf)?;

        let raw = encode_short_name(name);
        let mut rec = [0u8; DIR_ENTRY_SIZE];
        rec[0..11].copy_from_slice(&raw);
        rec[11] = ATTR_DIRECTORY;
        rec[20..22].copy_from_slice(&((new_cluster >> 16) as u16).to_le_bytes());
        rec[26..28].copy_from_slice(&(new_cluster as u16).to_le_bytes());
        self.add_dir_record(parent_cluster, &rec)?;
        Ok(new_cluster)
    }

    /// Generate a unique 8.3 short name for `long` inside `dir_cluster`.
    ///
    /// We only ever author lossy long names here (4-char `.wasm` ext,
    /// `limine.conf`, or a >8-char stem), so we always emit the numeric-tail
    /// (`BASIS~n`) form rather than a 1:1 8.3 copy. Steps:
    ///   1. Build the upper-cased "OEM-ish" stem/ext: keep `A-Z 0-9` and the
    ///      handful of punctuation chars the FAT spec allows in a short name,
    ///      map everything else to `_`, drop spaces, drop leading dots.
    ///   2. Split on the LAST `.` → stem + ext (3-char ext field).
    ///   3. For `n = 1, 2, 3, …`, form `<basis>~<n>` where `basis` is the first
    ///      `6 - extra` chars of the stem so the whole `~n` fits 8 bytes, pad to
    ///      the 11-byte field, and scan `dir_cluster`'s chain for a colliding
    ///      8.3 record (skipping free `0x00`/`0xE5` and LFN `0x0F` entries). The
    ///      first `n` with no collision wins.
    ///
    /// Returns the raw 11-byte short name (name[0..8] + ext[8..11], NO dot,
    /// space-padded).
    fn short_name(&mut self, long: &str, dir_cluster: u32) -> Result<[u8; 11], VfsError> {
        // --- 1. sanitise to an upper-cased OEM stem/ext --------------------
        // Allowed short-name chars besides A-Z / 0-9 (FAT spec long set).
        fn allowed(c: u8) -> bool {
            c.is_ascii_uppercase() || c.is_ascii_digit()
                || matches!(c, b'_' | b'~' | b'!' | b'@' | b'#' | b'$'
                    | b'%' | b'^' | b'&' | b'(' | b')' | b'-' | b'{' | b'}')
        }
        // Drop leading dots, then map each byte: space → skip, allowed → keep,
        // else → '_'. Dots are kept (they delimit the extension split below).
        let mut cleaned: Vec<u8> = Vec::with_capacity(long.len());
        let mut seen_non_dot = false;
        for &b in long.as_bytes() {
            let c = b.to_ascii_uppercase();
            if c == b' ' { continue; }
            if c == b'.' {
                if !seen_non_dot { continue; } // leading dot
                cleaned.push(b'.');
                continue;
            }
            seen_non_dot = true;
            cleaned.push(if allowed(c) { c } else { b'_' });
        }

        // --- 2. split stem / ext on the LAST '.' ---------------------------
        let (stem, ext): (&[u8], &[u8]) = match cleaned.iter().rposition(|&b| b == b'.') {
            Some(p) => (&cleaned[..p], &cleaned[p + 1..]),
            None => (&cleaned[..], &[][..]),
        };
        // The ext field holds the first 3 non-dot chars of the ext.
        let mut ext_field = [b' '; 3];
        for (i, &b) in ext.iter().filter(|&&b| b != b'.').take(3).enumerate() {
            ext_field[i] = b;
        }
        // The stem may still contain interior dots (e.g. "a.b.c" → stem "a.b");
        // strip them for the basis (short-name field is dot-free).
        let stem_nodot: Vec<u8> = stem.iter().copied().filter(|&b| b != b'.').collect();

        // --- 3. numeric-tail loop ------------------------------------------
        // `~n` costs `1 + digits(n)` bytes; basis takes the remaining 8.
        for n in 1u32..=999_999 {
            let tail = {
                let mut t = Vec::new();
                t.push(b'~');
                // decimal digits of n, no alloc::format (keep it OEM bytes).
                let mut buf = [0u8; 9];
                let mut len = 0;
                let mut v = n;
                if v == 0 { buf[len] = b'0'; len += 1; }
                while v > 0 { buf[len] = b'0' + (v % 10) as u8; v /= 10; len += 1; }
                for i in (0..len).rev() { t.push(buf[i]); }
                t
            };
            let basis_len = 8usize.saturating_sub(tail.len());
            let mut name_field = [b' '; 8];
            let take = stem_nodot.len().min(basis_len);
            name_field[..take].copy_from_slice(&stem_nodot[..take]);
            for (i, &b) in tail.iter().enumerate() {
                name_field[take + i] = b;
            }
            let mut short = [b' '; 11];
            short[0..8].copy_from_slice(&name_field);
            short[8..11].copy_from_slice(&ext_field);

            if !self.short_name_exists(dir_cluster, &short)? {
                return Ok(short);
            }
        }
        Err(VfsError::NoSpace)
    }

    /// Scan `dir_cluster`'s chain for a non-free, non-LFN record whose 11-byte
    /// 8.3 name equals `short`. Used by `short_name` for `~n` collision search.
    fn short_name_exists(&mut self, dir_cluster: u32, short: &[u8; 11])
        -> Result<bool, VfsError>
    {
        let chain = self.chain(dir_cluster)?;
        let cluster_bytes = self.bpb.cluster_bytes();
        let entries_per_cluster = cluster_bytes / DIR_ENTRY_SIZE;
        let mut buf = alloc::vec![0u8; cluster_bytes];
        for c in chain {
            self.read_cluster(c, &mut buf)?;
            for i in 0..entries_per_cluster {
                let rec = &buf[i * DIR_ENTRY_SIZE..(i + 1) * DIR_ENTRY_SIZE];
                let first = rec[0];
                if first == 0x00 { return Ok(false); } // end of directory
                if first == 0xE5 { continue; }          // deleted
                if rec[11] == ATTR_LFN { continue; }    // LFN sub-entry
                if &rec[0..11] == &short[..] { return Ok(true); }
            }
        }
        Ok(false)
    }

    /// LFN checksum of an 11-byte short name (FAT spec `ChkSum`).
    fn lfn_checksum(short: &[u8; 11]) -> u8 {
        let mut sum: u8 = 0;
        for &c in short {
            sum = ((sum & 1) << 7).wrapping_add(sum >> 1).wrapping_add(c);
        }
        sum
    }

    /// Build the LFN entry run for `long` (the on-disk records that precede the
    /// short entry). Returns them in **physical order** (highest sequence first,
    /// sequence 1 last — i.e. reverse of logical chunk order), so the caller can
    /// write `run ++ [short_record]` consecutively.
    ///
    /// Each LFN entry carries 13 UTF-16LE name units at byte offsets
    /// `1..11` (units 0..5), `14..26` (units 5..11), `28..32` (units 11..13).
    /// Past the name end we write a single `0x0000` terminator then pad with
    /// `0xFFFF`. Byte 11 = `0x0F` (LFN attr), 12 = 0 (type), 13 = checksum,
    /// 26..28 = 0 (first-cluster, always 0 for LFN).
    fn build_lfn_run(long: &str, short: &[u8; 11]) -> Vec<[u8; 32]> {
        let checksum = Self::lfn_checksum(short);
        // ASCII → one u16 unit each (our inputs are ASCII; non-ASCII would still
        // round-trip via char::encode_utf16 but we never author such names).
        let units: Vec<u16> = long.encode_utf16().collect();
        let n_entries = (units.len() + 12) / 13; // ceil(len / 13), >=1 here

        // Offsets within a 32-byte entry where the 13 name units live.
        const SLOTS: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];

        let mut logical: Vec<[u8; 32]> = Vec::with_capacity(n_entries);
        for k in 0..n_entries {
            let mut e = [0u8; 32];
            let mut seq = (k as u8) + 1;
            if k == n_entries - 1 { seq |= 0x40; }
            e[0] = seq;
            e[11] = ATTR_LFN;
            e[12] = 0x00;
            e[13] = checksum;
            e[26] = 0x00;
            e[27] = 0x00;

            // Fill the 13 unit slots for this chunk.
            let mut terminated = false;
            for j in 0..13 {
                let off = SLOTS[j];
                let idx = k * 13 + j;
                let val: u16 = if idx < units.len() {
                    units[idx]
                } else if !terminated {
                    terminated = true; // first padding slot = NUL terminator
                    0x0000
                } else {
                    0xFFFF
                };
                e[off..off + 2].copy_from_slice(&val.to_le_bytes());
            }
            logical.push(e);
        }

        // Physical order = reverse of logical.
        logical.reverse();
        logical
    }

    /// Insert a contiguous run of directory entries (`K` LFN + 1 short) into
    /// `dir_cluster`'s chain. The whole run MUST be physically consecutive and
    /// MUST NOT straddle a cluster boundary, so we search each cluster for
    /// `run.len()` consecutive free slots (first byte `0x00`/`0xE5`). If no
    /// cluster has a long-enough free run, allocate + link a fresh cluster and
    /// place the run at slot 0. Generalises `add_dir_record` (the K=1 case).
    fn add_dir_run(&mut self, dir_cluster: u32, run: &[[u8; 32]]) -> Result<(), VfsError> {
        if run.is_empty() { return Ok(()); }
        let need = run.len();
        let chain = self.chain(dir_cluster)?;
        let cluster_bytes = self.bpb.cluster_bytes();
        let entries_per_cluster = cluster_bytes / DIR_ENTRY_SIZE;
        // A run longer than a whole cluster can never be placed contiguously.
        if need > entries_per_cluster { return Err(VfsError::NoSpace); }
        let mut buf = alloc::vec![0u8; cluster_bytes];

        for &c in &chain {
            self.read_cluster(c, &mut buf)?;
            // Scan for a window of `need` consecutive free slots.
            let mut run_start: Option<usize> = None;
            let mut free_count = 0usize;
            for i in 0..entries_per_cluster {
                let first = buf[i * DIR_ENTRY_SIZE];
                if first == 0x00 || first == 0xE5 {
                    if free_count == 0 { run_start = Some(i); }
                    free_count += 1;
                    if free_count == need {
                        let start = run_start.unwrap();
                        for (j, ent) in run.iter().enumerate() {
                            let off = (start + j) * DIR_ENTRY_SIZE;
                            buf[off..off + DIR_ENTRY_SIZE].copy_from_slice(ent);
                        }
                        self.write_cluster(c, &buf)?;
                        return Ok(());
                    }
                } else {
                    free_count = 0;
                    run_start = None;
                }
            }
        }

        // No cluster had a long-enough consecutive free run — extend the chain.
        // The fresh cluster is zeroed (all slots free), so the run fits at 0.
        //
        // First, neutralise the old tail cluster's trailing `0x00` free slots.
        // A FAT directory may have at most ONE `0x00` "end" region, and it must
        // sit in the very last cluster. Once we link a new cluster after `last`,
        // any `0x00` slot still in `last` would make every reader
        // (`read_dir_entries` / `find_subdir` / `short_name_exists`) treat that
        // slot as end-of-directory and stop *before* the new cluster, silently
        // dropping its entries and blinding the collision scan. Rewrite those
        // free slots as `0xE5` (deleted) so readers skip them and traverse the
        // chain link. (`last` is the only cluster that can hold a `0x00` here:
        // the copy path is append-only, so every earlier extend already did the
        // same to its then-last cluster — last-cluster-only is sufficient.)
        let last = *chain.last().ok_or(VfsError::IoError)?;
        self.read_cluster(last, &mut buf)?;
        let mut dirty = false;
        for i in 0..entries_per_cluster {
            let off = i * DIR_ENTRY_SIZE;
            if buf[off] == 0x00 {
                buf[off] = 0xE5;
                dirty = true;
            }
        }
        if dirty {
            self.write_cluster(last, &buf)?;
        }

        let new_cluster = self.alloc_cluster()?;
        self.write_fat_entry(last, new_cluster)?;
        buf.iter_mut().for_each(|b| *b = 0);
        for (j, ent) in run.iter().enumerate() {
            buf[j * DIR_ENTRY_SIZE..(j + 1) * DIR_ENTRY_SIZE].copy_from_slice(ent);
        }
        self.write_cluster(new_cluster, &buf)?;
        Ok(())
    }

    /// Write a file with a (possibly long) name at `path` on the authored
    /// volume. Creates any missing parent directories (each a valid 8.3 short
    /// name, like `create_dirs`). The leaf gets a real LFN run so tools and
    /// Limine read its full long name.
    ///
    /// `bytes` empty → a zero-length file (first cluster 0, no allocation).
    /// Otherwise the whole cluster chain is bulk-allocated at once
    /// (`alloc_chain`, one batched FAT write) and the data streamed in large
    /// multi-sector writes over the contiguous run (`write_data_run`), with the
    /// final cluster's tail zero-padded so no stale bytes leak past EOF.
    pub fn write_file(&mut self, path: &str, bytes: &[u8]) -> Result<(), VfsError> {
        // --- split parent components + final name --------------------------
        let mut comps: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
        let name = comps.pop().ok_or(VfsError::InvalidPath)?;
        if name.is_empty() { return Err(VfsError::InvalidPath); }

        // --- walk/create the parent dirs (same rules as create_dirs) -------
        let mut parent = self.bpb.root_clus;
        for comp in comps {
            if comp.is_empty() || comp.len() > 8
                || !comp.bytes().all(|b| b.is_ascii_alphanumeric())
            {
                return Err(VfsError::IoError);
            }
            let want = comp.to_ascii_lowercase();
            parent = match self.find_subdir(parent, &want)? {
                Some(c) => c,
                None => self.mkdir(parent, comp)?,
            };
        }

        // --- build the file data cluster chain -----------------------------
        // Empty file → no allocation, first cluster 0. Otherwise bulk-allocate
        // the whole chain at once (one batched FAT write, O(n)) and stream the
        // data in large multi-sector writes over the contiguous run, instead of
        // the old per-cluster alloc_cluster + write_cluster (O(n^2) for a 20 MB
        // payload).
        let cluster_bytes = self.bpb.cluster_bytes();
        let first_cluster: u32 = if bytes.is_empty() {
            0
        } else {
            let num_clusters =
                ((bytes.len() + cluster_bytes - 1) / cluster_bytes) as u32;
            let first = self.alloc_chain(num_clusters)?;
            // Re-walk the chain we just linked to get the actual cluster list
            // (cheap: ~ceil(span/128) cached FAT-sector reads). This is correct
            // whether the run is contiguous (fast path) or fell back to a
            // scattered allocation, and lets the data writer batch contiguous
            // spans.
            let clusters = self.chain(first)?;
            self.write_data_run(&clusters, bytes)?;
            first
        };

        // --- 8.3 short record + LFN run ------------------------------------
        let short = self.short_name(name, parent)?;
        let mut run = Self::build_lfn_run(name, &short);

        let mut rec = [0u8; DIR_ENTRY_SIZE];
        rec[0..11].copy_from_slice(&short);
        rec[11] = ATTR_ARCHIVE;
        rec[20..22].copy_from_slice(&((first_cluster >> 16) as u16).to_le_bytes());
        rec[26..28].copy_from_slice(&(first_cluster as u16).to_le_bytes());
        rec[28..32].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
        run.push(rec);

        self.add_dir_run(parent, &run)
    }
}

/// Create directory paths on a freshly [`format`]ted FAT32 volume (borrow-based,
/// synchronous — for the disk authoring path, NOT the mounted VFS). `paths` are
/// absolute, created parents-first, e.g. `&["/EFI", "/EFI/BOOT"]`. An existing
/// *directory* component is reused (so listing both a parent and its child is
/// fine); a name collision with a file is an error. Each component must be a
/// simple 8.3 name (1..=8 ASCII alphanumerics) — anything `encode_short_name`
/// would silently mangle is rejected with `IoError` instead, so a bad path can't
/// create a wrong-named directory.
pub fn create_dirs(dev: &mut dyn BlockDevice, paths: &[&str]) -> Result<(), VfsError> {
    let mut w = FatWriter::open(dev)?;
    for path in paths {
        // Walk components from the root, creating any that are missing.
        let mut cur = w.bpb.root_clus;
        for comp in path.split('/').filter(|c| !c.is_empty()) {
            // Reject anything that isn't a plain ≤8-char ASCII-alphanumeric name:
            // short-name encoding would otherwise mangle it and the lookup key
            // (lower-cased here) would diverge from what was written.
            if comp.is_empty() || comp.len() > 8
                || !comp.bytes().all(|b| b.is_ascii_alphanumeric())
            {
                return Err(VfsError::IoError);
            }
            let want = comp.to_ascii_lowercase();
            cur = match w.find_subdir(cur, &want)? {
                Some(c) => c,                       // already there → descend
                None    => w.mkdir(cur, comp)?,     // create + descend
            };
        }
    }
    Ok(())
}

/// Convenience for the storage phase: take the ahci PORT0 and mount it.
pub fn mount_from_ahci_port(port: AhciPort) -> Result<(), VfsError> {
    let fs = Fat32Fs::from_ahci_port(port)?;
    crate::vfs::mount("/mnt", crate::vfs::fs::FsImpl::Fat32(fs))?;
    Ok(())
}

/// Build + mount a FAT32 volume on any block device at /mnt.
pub fn mount_from_blockdev(dev: alloc::boxed::Box<dyn crate::blockdev::BlockDevice + Send>) -> Result<(), VfsError> {
    let fs = Fat32Fs::from_blockdev(dev)?;
    crate::vfs::mount("/mnt", crate::vfs::fs::FsImpl::Fat32(fs))?;
    Ok(())
}

// Suppress "unused" on format macro re-export.
#[allow(dead_code)]
fn _format_keep_alive(s: &str) -> String { format!("{}", s) }
