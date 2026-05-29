//! In-RAM filesystem. Tree of inodes; each inode is `Arc<Mutex<TmpInode>>`.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::vfs::error::VfsError;
use crate::vfs::file::{File, FileImpl, OpenFlags, Whence};
use crate::vfs::fs::FileSystem;
use crate::vfs::devices::{ConsoleFile, NullFile, ZeroFile, PtySlaveFile};

#[derive(Copy, Clone)]
pub enum TmpKind { Dir, Reg, DevConsole, DevNull, DevZero, PtySlave(usize) }

pub struct TmpInode {
    pub kind: TmpKind,
    pub children: BTreeMap<String, Arc<Mutex<TmpInode>>>, // Dir-only
    pub content:  Vec<u8>,                                // Reg-only
}

impl TmpInode {
    fn new_dir() -> Self {
        Self { kind: TmpKind::Dir, children: BTreeMap::new(), content: Vec::new() }
    }
    fn new_reg() -> Self {
        Self { kind: TmpKind::Reg, children: BTreeMap::new(), content: Vec::new() }
    }
}

pub struct Tmpfs {
    pub root: Arc<Mutex<TmpInode>>,
}

impl Tmpfs {
    pub fn new() -> Self {
        Self { root: Arc::new(Mutex::new(TmpInode::new_dir())) }
    }

    /// Create a directory at the given (already-split) path. Parent must exist.
    pub fn mkdir(&self, path: &[&str]) -> Result<(), VfsError> {
        let (parent, name) = self.parent_and_name(path)?;
        let mut p = parent.lock();
        if !matches!(p.kind, TmpKind::Dir) { return Err(VfsError::NotDirectory); }
        if p.children.contains_key(name) { return Err(VfsError::AlreadyExists); }
        p.children.insert(name.to_string(), Arc::new(Mutex::new(TmpInode::new_dir())));
        Ok(())
    }

    /// Insert a pre-built inode (used to seed device files at boot).
    pub fn insert_inode(&self, path: &[&str], inode: TmpInode) -> Result<(), VfsError> {
        let (parent, name) = self.parent_and_name(path)?;
        let mut p = parent.lock();
        if !matches!(p.kind, TmpKind::Dir) { return Err(VfsError::NotDirectory); }
        if p.children.contains_key(name) { return Err(VfsError::AlreadyExists); }
        p.children.insert(name.to_string(), Arc::new(Mutex::new(inode)));
        Ok(())
    }

    fn parent_and_name<'a>(&self, path: &'a [&'a str])
        -> Result<(Arc<Mutex<TmpInode>>, &'a str), VfsError>
    {
        if path.is_empty() { return Err(VfsError::InvalidPath); }
        let (last, rest) = path.split_last().unwrap();
        let parent = self.walk(rest)?;
        Ok((parent, *last))
    }

    fn walk(&self, components: &[&str]) -> Result<Arc<Mutex<TmpInode>>, VfsError> {
        let mut cur = self.root.clone();
        for c in components {
            let next = {
                let node = cur.lock();
                if !matches!(node.kind, TmpKind::Dir) { return Err(VfsError::NotDirectory); }
                node.children.get(*c).cloned().ok_or(VfsError::NotFound)?
            };
            cur = next;
        }
        Ok(cur)
    }
}

impl FileSystem for Tmpfs {
    async fn open(&self, path: &[&str], flags: OpenFlags) -> Result<FileImpl, VfsError> {
        // Fast path: try to walk first. Common case for non-creating opens.
        if let Ok(node) = self.walk(path) {
            let kind = node.lock().kind;
            return match kind {
                TmpKind::Dir => Err(VfsError::IsDirectory),
                TmpKind::Reg => Ok(FileImpl::Tmp(TmpfsFile { node, pos: 0 })),
                TmpKind::DevConsole     => Ok(FileImpl::Console(ConsoleFile)),
                TmpKind::DevNull        => Ok(FileImpl::Null(NullFile)),
                TmpKind::DevZero        => Ok(FileImpl::Zero(ZeroFile)),
                TmpKind::PtySlave(idx)  => Ok(FileImpl::PtySlave(PtySlaveFile { idx })),
            };
        }
        if !flags.contains(OpenFlags::CREATE) {
            return Err(VfsError::NotFound);
        }
        // CREATE path: acquire the parent lock *first*, then double-check
        // inside the lock that the child still doesn't exist. This closes
        // the TOCTOU race between the walk above and the insert below —
        // a race that became reachable with Step 10.5 cooperative fibers
        // (multiple fibers can be mid-open concurrently).
        let (parent, name) = self.parent_and_name(path)?;
        let mut p = parent.lock();
        if !matches!(p.kind, TmpKind::Dir) {
            return Err(VfsError::NotDirectory);
        }
        // Concurrent insert: another fiber created the same path while
        // we were between the walk and this lock. Adopt its inode.
        if let Some(existing) = p.children.get(name).cloned() {
            drop(p);
            let kind = existing.lock().kind;
            return match kind {
                TmpKind::Dir => Err(VfsError::IsDirectory),
                TmpKind::Reg => Ok(FileImpl::Tmp(TmpfsFile { node: existing, pos: 0 })),
                TmpKind::DevConsole     => Ok(FileImpl::Console(ConsoleFile)),
                TmpKind::DevNull        => Ok(FileImpl::Null(NullFile)),
                TmpKind::DevZero        => Ok(FileImpl::Zero(ZeroFile)),
                TmpKind::PtySlave(idx)  => Ok(FileImpl::PtySlave(PtySlaveFile { idx })),
            };
        }
        let arc = Arc::new(Mutex::new(TmpInode::new_reg()));
        p.children.insert(name.to_string(), arc.clone());
        drop(p);
        Ok(FileImpl::Tmp(TmpfsFile { node: arc, pos: 0 }))
    }

    async fn create(&self, path: &[&str]) -> Result<(), VfsError> {
        let (parent, name) = self.parent_and_name(path)?;
        let mut p = parent.lock();
        if !matches!(p.kind, TmpKind::Dir) { return Err(VfsError::NotDirectory); }
        if p.children.contains_key(name) { return Err(VfsError::AlreadyExists); }
        p.children.insert(name.to_string(), Arc::new(Mutex::new(TmpInode::new_reg())));
        Ok(())
    }

    async fn unlink(&self, path: &[&str]) -> Result<(), VfsError> {
        let (parent, name) = self.parent_and_name(path)?;
        let mut p = parent.lock();
        let child = p.children.get(name).cloned().ok_or(VfsError::NotFound)?;
        if matches!(child.lock().kind, TmpKind::Dir) {
            return Err(VfsError::IsDirectory);
        }
        p.children.remove(name);
        Ok(())
    }

    async fn readdir(&self, path: &[&str]) -> Result<Vec<crate::vfs::fs::VfsDirent>, VfsError> {
        use crate::vfs::fs::{VfsDirent, VfsKind};
        let node = self.walk(path)?;
        let g = node.lock();
        if !matches!(g.kind, TmpKind::Dir) {
            return Err(VfsError::NotDirectory);
        }
        let mut out: Vec<VfsDirent> = Vec::with_capacity(g.children.len());
        for (name, inode) in g.children.iter() {
            let kind = match inode.lock().kind {
                TmpKind::Dir => VfsKind::Dir,
                TmpKind::Reg => VfsKind::Reg,
                TmpKind::DevConsole | TmpKind::DevNull | TmpKind::DevZero
                | TmpKind::PtySlave(_) => VfsKind::Device,
            };
            out.push(VfsDirent { name: name.clone(), kind });
        }
        Ok(out)
    }

    async fn stat(&self, path: &[&str]) -> Result<crate::vfs::fs::VfsStat, VfsError> {
        use crate::vfs::fs::{VfsStat, VfsKind};
        let node = self.walk(path)?;
        let g = node.lock();
        let (kind, size) = match g.kind {
            TmpKind::Dir => (VfsKind::Dir, 0u64),
            TmpKind::Reg => (VfsKind::Reg, g.content.len() as u64),
            TmpKind::DevConsole | TmpKind::DevNull | TmpKind::DevZero
            | TmpKind::PtySlave(_) => (VfsKind::Device, 0u64),
        };
        Ok(VfsStat { kind, size })
    }
}

pub struct TmpfsFile {
    pub node: Arc<Mutex<TmpInode>>,
    pub pos:  u64,
}

impl File for TmpfsFile {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, VfsError> {
        let node = self.node.lock();
        let start = self.pos as usize;
        if start >= node.content.len() { return Ok(0); }
        let n = core::cmp::min(buf.len(), node.content.len() - start);
        buf[..n].copy_from_slice(&node.content[start..start + n]);
        drop(node);
        self.pos += n as u64;
        Ok(n)
    }
    async fn write(&mut self, buf: &[u8]) -> Result<usize, VfsError> {
        let mut node = self.node.lock();
        let start = self.pos as usize;
        // Guard against integer wrap on huge positions / lengths.
        let end = start.checked_add(buf.len()).ok_or(VfsError::NoSpace)?;
        if node.content.len() < end { node.content.resize(end, 0); }
        node.content[start..end].copy_from_slice(buf);
        drop(node);
        self.pos += buf.len() as u64;
        Ok(buf.len())
    }
    async fn seek(&mut self, off: i64, whence: Whence) -> Result<u64, VfsError> {
        let len = self.node.lock().content.len() as i64;
        let base = match whence {
            Whence::Set => 0,
            Whence::Cur => self.pos as i64,
            Whence::End => len,
        };
        let new = base.checked_add(off).ok_or(VfsError::Invalid)?;
        if new < 0 { return Err(VfsError::Invalid); }
        self.pos = new as u64;
        Ok(self.pos)
    }
    async fn stat(&self) -> Result<crate::vfs::fs::VfsStat, VfsError> {
        let g = self.node.lock();
        let (kind, size) = match g.kind {
            TmpKind::Reg => (crate::vfs::fs::VfsKind::Reg, g.content.len() as u64),
            TmpKind::Dir => (crate::vfs::fs::VfsKind::Dir, 0),
            _ => (crate::vfs::fs::VfsKind::Device, 0),
        };
        Ok(crate::vfs::fs::VfsStat { kind, size })
    }
}
