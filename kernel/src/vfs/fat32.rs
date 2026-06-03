//! Minimal in-tree FAT32 filesystem.
//!
//! Read + write. Short-name only (no LFN handling — files are looked up by
//! their 8.3 form, lowercase normalised). Single FAT mirror updated.
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
    name:      String,       // normalised 8.3 lowercase ("hello.txt")
    is_dir:    bool,
    cluster:   u32,
    size:      u32,
    /// Sector containing the on-disk 32-byte record.
    rec_sec:   u64,
    /// Byte offset of the record within `rec_sec`.
    rec_off:   usize,
}

/// Read every short-name entry inside the directory starting at `start_cluster`.
fn read_dir_entries(inner: &mut Inner, start_cluster: u32) -> Result<Vec<DirEntry>, VfsError> {
    let chain = inner.chain(start_cluster)?;
    let cluster_bytes = inner.bpb.cluster_bytes();
    let mut out = Vec::new();
    let mut buf = alloc::vec![0u8; cluster_bytes];
    for c in chain {
        inner.read_cluster(c, &mut buf)?;
        let entries_per_cluster = cluster_bytes / DIR_ENTRY_SIZE;
        for i in 0..entries_per_cluster {
            let rec = &buf[i * DIR_ENTRY_SIZE..(i+1) * DIR_ENTRY_SIZE];
            let first = rec[0];
            if first == 0x00 { return Ok(out); }            // end of directory
            if first == 0xE5 { continue; }                  // deleted
            let attr = rec[11];
            if attr == ATTR_LFN { continue; }               // LFN sub-entry
            if attr & ATTR_VOLUME_ID != 0 { continue; }     // volume label
            let name = decode_short_name(&rec[0..11]);
            if name.is_empty() { continue; }
            let cluster_hi = u16::from_le_bytes([rec[20], rec[21]]) as u32;
            let cluster_lo = u16::from_le_bytes([rec[26], rec[27]]) as u32;
            let cluster = (cluster_hi << 16) | cluster_lo;
            let size = u32::from_le_bytes([rec[28], rec[29], rec[30], rec[31]]);
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
    // Free count = data clusters minus cluster 2 (root, in use).
    let free_count: u32 = clusters - 1;
    fsinfo[488..492].copy_from_slice(&free_count.to_le_bytes());
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
