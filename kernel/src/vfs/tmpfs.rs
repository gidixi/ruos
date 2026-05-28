//! In-RAM filesystem. Tree of inodes; each inode is `Arc<Mutex<TmpInode>>`.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::vfs::error::VfsError;
use crate::vfs::file::{File, FileImpl, OpenFlags, Whence};
use crate::vfs::fs::FileSystem;
use crate::vfs::devices::{ConsoleFile, NullFile, ZeroFile};

#[derive(Copy, Clone)]
pub enum TmpKind { Dir, Reg, DevConsole, DevNull, DevZero }

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
        let node = match self.walk(path) {
            Ok(n) => n,
            Err(VfsError::NotFound) if flags.contains(OpenFlags::CREATE) => {
                let (parent, name) = self.parent_and_name(path)?;
                let mut p = parent.lock();
                p.children.insert(name.to_string(),
                    Arc::new(Mutex::new(TmpInode::new_reg())));
                p.children.get(name).cloned().unwrap()
            }
            Err(e) => return Err(e),
        };
        let kind = node.lock().kind;
        match kind {
            TmpKind::Dir => Err(VfsError::IsDirectory),
            TmpKind::Reg => Ok(FileImpl::Tmp(TmpfsFile { node, pos: 0 })),
            TmpKind::DevConsole => Ok(FileImpl::Console(ConsoleFile)),
            TmpKind::DevNull    => Ok(FileImpl::Null(NullFile)),
            TmpKind::DevZero    => Ok(FileImpl::Zero(ZeroFile)),
        }
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
        p.children.remove(name).ok_or(VfsError::NotFound)?;
        Ok(())
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
        let end = start + buf.len();
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
        let new = base + off;
        if new < 0 { return Err(VfsError::InvalidPath); }
        self.pos = new as u64;
        Ok(self.pos)
    }
}
