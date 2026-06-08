//! Filesystem ISO9660 read-only (live-CD).
//!
//! Supporta il sufficiente per leggere `/bin/*.wasm`/`.cwasm` dal CD di boot:
//! Primary Volume Descriptor (settore 16), traversata directory via directory
//! record, file come extent contiguo. Niente Joliet/Rock Ridge nel primo taglio
//! (nomi 8.3 + `;1`). Backed da qualunque `BlockDevice` a 2048 B.
//!
//! Tutte le scritture → `VfsError::Unsupported` (CD read-only).

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::blockdev::{BlockDevice, BlockError};
use crate::vfs::error::VfsError;
use crate::vfs::file::{File, FileImpl, OpenFlags, Whence};
use crate::vfs::fs::{FileSystem, VfsDirent, VfsKind, VfsStat};

const ISO_SECTOR: usize = 2048;
const PVD_LBA: u64 = 16;

/// Un directory record ISO9660 parsato (solo i campi che usiamo).
#[derive(Debug, Clone)]
struct IsoEntry {
    name: String,   // normalizzato lowercase, senza ";1"
    is_dir: bool,
    extent_lba: u32,
    size: u32,
}

struct IsoInner {
    dev: Box<dyn BlockDevice + Send>,
    root_lba: u32,
    root_size: u32,
    /// Componenti del sottopercorso ISO da anteporre (es. ["bin"]) — la mount
    /// mappa il prefisso VFS `/bin` su `/bin` dell'ISO.
    base: Vec<String>,
}

impl IsoInner {
    fn read_sector(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), VfsError> {
        self.dev.read_blocks(lba, buf).map_err(map_block_err)
    }

    /// Legge l'extent (`size` byte a partire da `lba`) in un Vec.
    fn read_extent(&mut self, lba: u32, size: u32) -> Result<Vec<u8>, VfsError> {
        let sectors = ((size as usize) + ISO_SECTOR - 1) / ISO_SECTOR;
        let mut out = alloc::vec![0u8; sectors * ISO_SECTOR];
        for i in 0..sectors {
            let s = &mut out[i * ISO_SECTOR..(i + 1) * ISO_SECTOR];
            self.read_sector(lba as u64 + i as u64, s)?;
        }
        out.truncate(size as usize);
        Ok(out)
    }
}

/// Parsa un blocco di directory record (l'extent di una dir) in voci.
/// I record non attraversano i confini di settore: un `len==0` salta al
/// prossimo confine di 2048 B.
fn parse_dir(extent: &[u8]) -> Vec<IsoEntry> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off < extent.len() {
        let len = extent[off] as usize;
        if len == 0 {
            let next = (off / ISO_SECTOR + 1) * ISO_SECTOR;
            if next <= off { break; }
            off = next;
            continue;
        }
        if off + len > extent.len() || len < 33 { break; }
        let rec = &extent[off..off + len];
        let extent_lba = u32::from_le_bytes([rec[2], rec[3], rec[4], rec[5]]);
        let size = u32::from_le_bytes([rec[10], rec[11], rec[12], rec[13]]);
        let flags = rec[25];
        let is_dir = flags & 0x02 != 0;
        let name_len = rec[32] as usize;
        if 33 + name_len > len { off += len; continue; }
        let raw = &rec[33..33 + name_len];
        if name_len == 1 && (raw[0] == 0 || raw[0] == 1) {
            off += len;
            continue;
        }
        // Limine's xorriso writes Rock Ridge (SUSP): the on-disk ISO9660 name is
        // a mangled 8.3 (`SHELL.WAS;1`, `.wasm` ext > 3 chars breaks level 1),
        // while the real POSIX name (`shell.wasm`) lives in the Rock Ridge `NM`
        // entry of the System Use Area. Prefer NM; fall back to the 8.3 name.
        let sua_start = 33 + name_len + (1 - (name_len & 1));
        let rr = if sua_start <= len { rock_ridge_name(&rec[sua_start..]) } else { None };
        let name = rr.unwrap_or_else(|| {
            let mut n = String::from_utf8_lossy(raw).into_owned();
            if let Some(p) = n.find(';') { n.truncate(p); }
            n.to_ascii_lowercase()
        });
        out.push(IsoEntry { name, is_dir, extent_lba, size });
        off += len;
    }
    out
}

/// Extract the Rock Ridge alternate name ("NM" SUSP entries) from a directory
/// record's System Use Area. Concatenates split NM entries. Returns `None` when
/// no usable NM entry is present (caller falls back to the 8.3 name).
fn rock_ridge_name(sua: &[u8]) -> Option<String> {
    let mut name = String::new();
    let mut found = false;
    let mut i = 0usize;
    while i + 4 <= sua.len() {
        let (s0, s1) = (sua[i], sua[i + 1]);
        let elen = sua[i + 2] as usize;
        if elen < 4 || i + elen > sua.len() { break; }
        if s0 == b'S' && s1 == b'T' { break; } // SUSP terminator
        if s0 == b'N' && s1 == b'M' {
            let nm_flags = sua[i + 4];
            // Skip CURRENT (0x02) / PARENT (0x04) self/parent name entries.
            if nm_flags & 0x06 == 0 && elen > 5 {
                name.push_str(&String::from_utf8_lossy(&sua[i + 5..i + elen]));
                found = true;
            }
        }
        i += elen;
    }
    if found { Some(name.to_ascii_lowercase()) } else { None }
}

fn map_block_err(_e: BlockError) -> VfsError { VfsError::IoError }

pub struct Iso9660Fs {
    inner: Arc<Mutex<IsoInner>>,
}

impl Iso9660Fs {
    /// Monta un ISO9660 da `dev`, esponendo il sottoalbero `base` (es. "/bin").
    pub fn from_blockdev(
        mut dev: Box<dyn BlockDevice + Send>,
        base: &str,
    ) -> Result<Self, VfsError> {
        let mut pvd = [0u8; ISO_SECTOR];
        dev.read_blocks(PVD_LBA, &mut pvd).map_err(map_block_err)?;
        if pvd[0] != 1 || &pvd[1..6] != b"CD001" {
            return Err(VfsError::IoError);
        }
        let root = &pvd[156..156 + 34];
        let root_lba = u32::from_le_bytes([root[2], root[3], root[4], root[5]]);
        let root_size = u32::from_le_bytes([root[10], root[11], root[12], root[13]]);
        let base: Vec<String> = base.split('/').filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_lowercase()).collect();
        crate::binfo!("iso9660", "PVD ok root_lba={} root_size={} base={:?}",
            root_lba, root_size, base);
        Ok(Self { inner: Arc::new(Mutex::new(IsoInner {
            dev, root_lba, root_size, base,
        })) })
    }

    /// Risolve i componenti (sotto `base`) a una voce.
    fn lookup(&self, parts: &[&str]) -> Result<IsoEntry, VfsError> {
        let mut inner = self.inner.lock();
        let mut full: Vec<String> = inner.base.clone();
        full.extend(parts.iter().map(|s| s.to_ascii_lowercase()));

        let mut cur_lba = inner.root_lba;
        let mut cur_size = inner.root_size;
        let mut last: Option<IsoEntry> = None;
        for (i, comp) in full.iter().enumerate() {
            let extent = inner.read_extent(cur_lba, cur_size)?;
            let entries = parse_dir(&extent);
            let found = entries.into_iter().find(|e| &e.name == comp)
                .ok_or(VfsError::NotFound)?;
            let is_last = i == full.len() - 1;
            if !is_last {
                if !found.is_dir { return Err(VfsError::NotDirectory); }
                cur_lba = found.extent_lba;
                cur_size = found.size;
            }
            last = Some(found);
        }
        last.ok_or(VfsError::NotFound)
    }
}

impl FileSystem for Iso9660Fs {
    async fn open(&self, path: &[&str], _flags: OpenFlags) -> Result<FileImpl, VfsError> {
        let e = self.lookup(path)?;
        if e.is_dir { return Err(VfsError::IsDirectory); }
        Ok(FileImpl::Iso9660(Iso9660File {
            fs: Arc::clone(&self.inner),
            extent_lba: e.extent_lba,
            size: e.size,
            pos: 0,
        }))
    }

    async fn create(&self, _path: &[&str]) -> Result<(), VfsError> { Err(VfsError::Unsupported) }
    async fn unlink(&self, _path: &[&str]) -> Result<(), VfsError> { Err(VfsError::Unsupported) }
    async fn mkdir(&self, _path: &[&str]) -> Result<(), VfsError> { Err(VfsError::Unsupported) }
    async fn rmdir(&self, _path: &[&str]) -> Result<(), VfsError> { Err(VfsError::Unsupported) }
    async fn rename(&self, _src: &[&str], _dst: &[&str]) -> Result<(), VfsError> { Err(VfsError::Unsupported) }

    async fn readdir(&self, path: &[&str]) -> Result<Vec<VfsDirent>, VfsError> {
        let (lba, size) = if path.is_empty() {
            let inner = self.inner.lock();
            if inner.base.is_empty() {
                (inner.root_lba, inner.root_size)
            } else {
                drop(inner);
                let e = self.lookup(&[])?;
                (e.extent_lba, e.size)
            }
        } else {
            let e = self.lookup(path)?;
            if !e.is_dir { return Err(VfsError::NotDirectory); }
            (e.extent_lba, e.size)
        };
        let mut inner = self.inner.lock();
        let extent = inner.read_extent(lba, size)?;
        Ok(parse_dir(&extent).into_iter().map(|e| VfsDirent {
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

pub struct Iso9660File {
    fs: Arc<Mutex<IsoInner>>,
    extent_lba: u32,
    size: u32,
    pos: u64,
}

impl File for Iso9660File {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, VfsError> {
        if self.pos >= self.size as u64 || buf.is_empty() { return Ok(0); }
        let mut inner = self.fs.lock();
        let sector_in_file = self.pos / ISO_SECTOR as u64;
        let within = (self.pos % ISO_SECTOR as u64) as usize;
        let mut sbuf = [0u8; ISO_SECTOR];
        inner.read_sector(self.extent_lba as u64 + sector_in_file, &mut sbuf)?;
        let avail_in_sector = ISO_SECTOR - within;
        let avail_in_file = (self.size as u64 - self.pos) as usize;
        let n = buf.len().min(avail_in_sector).min(avail_in_file);
        buf[..n].copy_from_slice(&sbuf[within..within + n]);
        self.pos += n as u64;
        Ok(n)
    }

    async fn write(&mut self, _buf: &[u8]) -> Result<usize, VfsError> { Err(VfsError::Unsupported) }

    async fn seek(&mut self, off: i64, whence: Whence) -> Result<u64, VfsError> {
        let base = match whence {
            Whence::Set => 0i64,
            Whence::Cur => self.pos as i64,
            Whence::End => self.size as i64,
        };
        let np = base.checked_add(off).ok_or(VfsError::InvalidPath)?;
        if np < 0 { return Err(VfsError::InvalidPath); }
        self.pos = np as u64;
        Ok(self.pos)
    }

    async fn stat(&self) -> Result<VfsStat, VfsError> {
        Ok(VfsStat { kind: VfsKind::Reg, size: self.size as u64 })
    }
}

/// Monta un ISO9660 da un block device esponendo `iso_base` al prefisso `prefix`.
pub fn mount_from_blockdev(
    dev: Box<dyn BlockDevice + Send>,
    prefix: &str,
    iso_base: &str,
) -> Result<(), VfsError> {
    let fs = Iso9660Fs::from_blockdev(dev, iso_base)?;
    crate::vfs::mount(prefix, crate::vfs::fs::FsImpl::Iso9660(fs))
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec; use std::vec::Vec as StdVec;

    struct MemCd(StdVec<u8>);
    impl BlockDevice for MemCd {
        fn block_size(&self) -> u32 { 2048 }
        fn block_count(&self) -> u64 { (self.0.len() / 2048) as u64 }
        fn read_blocks(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
            let o = (lba as usize) * 2048;
            buf.copy_from_slice(&self.0[o..o + buf.len()]); Ok(())
        }
        fn write_blocks(&mut self, _lba: u64, _buf: &[u8]) -> Result<(), BlockError> {
            Err(BlockError::Io)
        }
    }

    fn dir_record(name: &[u8], lba: u32, size: u32, is_dir: bool) -> StdVec<u8> {
        let len = 33 + name.len();
        let len = if len % 2 == 1 { len + 1 } else { len };
        let mut r = vec![0u8; len];
        r[0] = len as u8;
        r[2..6].copy_from_slice(&lba.to_le_bytes());
        r[10..14].copy_from_slice(&size.to_le_bytes());
        r[25] = if is_dir { 0x02 } else { 0x00 };
        r[32] = name.len() as u8;
        r[33..33 + name.len()].copy_from_slice(name);
        r
    }

    fn build_iso() -> StdVec<u8> {
        let mut img = vec![0u8; 2048 * 24];
        img[16 * 2048] = 1;
        img[16 * 2048 + 1..16 * 2048 + 6].copy_from_slice(b"CD001");
        let root = dir_record(&[0u8], 20, 2048, true);
        img[16 * 2048 + 156..16 * 2048 + 156 + root.len()].copy_from_slice(&root);
        let mut off = 20 * 2048;
        let bin = dir_record(b"BIN", 21, 2048, true);
        img[off..off + bin.len()].copy_from_slice(&bin); off += bin.len();
        let _ = off;
        let ls = dir_record(b"LS.WASM;1", 22, 5, false);
        img[21 * 2048..21 * 2048 + ls.len()].copy_from_slice(&ls);
        img[22 * 2048..22 * 2048 + 5].copy_from_slice(b"hello");
        img
    }

    #[test] fn parse_pvd_and_lookup_file() {
        let fs = Iso9660Fs::from_blockdev(Box::new(MemCd(build_iso())), "/bin").unwrap();
        let e = fs.lookup(&["ls.wasm"]).unwrap();
        assert!(!e.is_dir);
        assert_eq!(e.extent_lba, 22);
        assert_eq!(e.size, 5);
    }

    #[test] fn lookup_missing_is_notfound() {
        let fs = Iso9660Fs::from_blockdev(Box::new(MemCd(build_iso())), "/bin").unwrap();
        assert!(matches!(fs.lookup(&["nope.wasm"]), Err(VfsError::NotFound)));
    }

    #[test] fn bad_magic_rejected() {
        let img = vec![0u8; 2048 * 20];
        assert!(Iso9660Fs::from_blockdev(Box::new(MemCd(img)), "/bin").is_err());
    }
}
