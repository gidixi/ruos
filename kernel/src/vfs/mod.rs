//! Async VFS API + tmpfs in-RAM filesystem + device files.

pub mod error;
pub mod path;
pub mod file;
pub mod fs;
pub mod fd;
pub mod block_on;
pub mod tmpfs;
pub mod devices;

pub use block_on::block_on;
pub use error::VfsError;
pub use file::{Fd, OpenFlags, Whence};

// Real API (open/close/read/write/seek/init/mount) lands in Task 3.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

use crate::vfs::fs::FsImpl;
use crate::vfs::tmpfs::{Tmpfs, TmpInode, TmpKind};

pub(crate) static MOUNTS: Mutex<Vec<(String, FsImpl)>> = Mutex::new(Vec::new());

pub fn mount(prefix: &str, fs: FsImpl) -> Result<(), VfsError> {
    if !prefix.starts_with('/') { return Err(VfsError::InvalidPath); }
    MOUNTS.lock().push((prefix.to_string(), fs));
    Ok(())
}

/// Build the in-RAM root tmpfs, mount it at `/`, populate /dev + /tmp.
/// Returns `AlreadyExists` if called twice (single init by design).
pub fn init() -> Result<usize, VfsError> {
    if !MOUNTS.lock().is_empty() { return Err(VfsError::AlreadyExists); }
    let fs = Tmpfs::new();
    fs.mkdir(&["dev"])?;
    fs.mkdir(&["tmp"])?;
    fs.insert_inode(&["dev", "console"], TmpInode {
        kind: TmpKind::DevConsole,
        children: alloc::collections::BTreeMap::new(),
        content: alloc::vec::Vec::new(),
    })?;
    fs.insert_inode(&["dev", "null"], TmpInode {
        kind: TmpKind::DevNull,
        children: alloc::collections::BTreeMap::new(),
        content: alloc::vec::Vec::new(),
    })?;
    fs.insert_inode(&["dev", "zero"], TmpInode {
        kind: TmpKind::DevZero,
        children: alloc::collections::BTreeMap::new(),
        content: alloc::vec::Vec::new(),
    })?;
    mount("/", FsImpl::Tmpfs(fs))?;
    Ok(MOUNTS.lock().len())
}
