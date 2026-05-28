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

use crate::vfs::fd::{FDS, allocate as fd_allocate, close as fd_close};

/// Locate the FsImpl covering `abspath` and return the components below the
/// mount point. Longest-prefix match.
fn resolve<'a>(abspath: &'a [&'a str]) -> Result<(usize, Vec<&'a str>), VfsError> {
    // For now: single mount at "/". Components match the full split.
    let mounts = MOUNTS.lock();
    if mounts.is_empty() { return Err(VfsError::NotFound); }
    // Index 0 is the root mount; "/" prefix always matches.
    Ok((0usize, abspath.to_vec()))
}

pub async fn open(path: &str, flags: OpenFlags) -> Result<Fd, VfsError> {
    let parts = path::split(path)?;
    let (idx, sub) = resolve(&parts)?;
    let mounts = MOUNTS.lock();
    let fs = &mounts[idx].1;
    let file = fs.open(&sub, flags).await?;
    drop(mounts);
    Ok(fd_allocate(file))
}

pub async fn close(fd: Fd) -> Result<(), VfsError> {
    fd_close(fd)
}

pub async fn read(fd: Fd, buf: &mut [u8]) -> Result<usize, VfsError> {
    // Hold the FDS lock across the inner await: all current File impls
    // (tmpfs, devices) resolve in a single poll, so no real suspension
    // occurs and the lock is released before the outer block_on returns.
    // When Step 9 brings an executor that can suspend, this needs the
    // take-and-restore pattern instead.
    let mut t = FDS.lock();
    let slot = t.get_mut(fd as usize).ok_or(VfsError::BadFd)?
        .as_mut().ok_or(VfsError::BadFd)?;
    slot.file.read(buf).await
}

pub async fn write(fd: Fd, buf: &[u8]) -> Result<usize, VfsError> {
    // Same FDS-lock-across-await caveat as `read` (see comment there).
    let mut t = FDS.lock();
    let slot = t.get_mut(fd as usize).ok_or(VfsError::BadFd)?
        .as_mut().ok_or(VfsError::BadFd)?;
    slot.file.write(buf).await
}

pub async fn seek(fd: Fd, off: i64, whence: Whence) -> Result<u64, VfsError> {
    // Same FDS-lock-across-await caveat as `read` (see comment there).
    let mut t = FDS.lock();
    let slot = t.get_mut(fd as usize).ok_or(VfsError::BadFd)?
        .as_mut().ok_or(VfsError::BadFd)?;
    slot.file.seek(off, whence).await
}
