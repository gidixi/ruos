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
pub use fs::{VfsDirent, VfsKind, VfsStat};

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
    fs.mkdir(&["bin"])?;
    fs.mkdir(&["etc"])?;
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
    fs.mkdir(&["dev", "pts"])?;
    const PTY_NAMES: [&str; crate::pty::NUM_PAIRS] = ["0", "1", "2", "3"];
    for (i, name) in PTY_NAMES.iter().enumerate() {
        fs.insert_inode(&["dev", "pts", name], TmpInode {
            kind: TmpKind::PtySlave(i),
            children: alloc::collections::BTreeMap::new(),
            content: alloc::vec::Vec::new(),
        })?;
    }
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

// Take-and-restore pattern: extract the FdEntry out of FDS, drop the
// global lock, perform the I/O (which may suspend cooperatively under
// Step 10.5 fibers), then restore the entry. Each VFS Fd is owned by
// exactly one fiber at a time, so the gap between take and restore is
// safe — no other code can observe the missing slot through normal
// vfs API calls. A concurrent `close(fd)` while we're awaiting will
// race: the slot will be None at restore time and we drop the file
// silently (equivalent to closing during the I/O).
async fn with_fd_take<F, T, FN>(fd: Fd, op: FN) -> Result<T, VfsError>
where
    FN: FnOnce(crate::vfs::fd::FdEntry) -> F,
    F: core::future::Future<Output = (crate::vfs::fd::FdEntry, Result<T, VfsError>)>,
{
    let entry = {
        let mut t = FDS.lock();
        t.get_mut(fd as usize).and_then(|s| s.take()).ok_or(VfsError::BadFd)?
    };
    let (entry, result) = op(entry).await;
    {
        let mut t = FDS.lock();
        if let Some(s) = t.get_mut(fd as usize) {
            if s.is_none() {
                // Slot still ours — restore. (If a concurrent close()
                // already nilled it, leaving s as None and dropping
                // `entry` matches the close-during-I/O semantics.)
                *s = Some(entry);
            } else {
                // Slot reused by an open() that happened during the
                // await window. Drop our entry; the new owner stays.
                drop(entry);
            }
        }
    }
    result
}

pub async fn read(fd: Fd, buf: &mut [u8]) -> Result<usize, VfsError> {
    with_fd_take(fd, |mut entry| async move {
        let r = entry.file.read(buf).await;
        (entry, r)
    }).await
}

pub async fn write(fd: Fd, buf: &[u8]) -> Result<usize, VfsError> {
    with_fd_take(fd, |mut entry| async move {
        let r = entry.file.write(buf).await;
        (entry, r)
    }).await
}

pub async fn seek(fd: Fd, off: i64, whence: Whence) -> Result<u64, VfsError> {
    with_fd_take(fd, |mut entry| async move {
        let r = entry.file.seek(off, whence).await;
        (entry, r)
    }).await
}

/// Stat the file backing `fd` without changing its read cursor. Used by
/// `wasi_snapshot_preview1::fd_filestat_get`. Closes Step 11 F7.
pub async fn stat_fd(fd: Fd) -> Result<VfsStat, VfsError> {
    with_fd_take(fd, |entry| async move {
        let r = entry.file.stat().await;
        (entry, r)
    }).await
}

/// List directory entries at `path`. Single-shot — no streaming cookie.
/// Holds the MOUNTS lock only for the brief lookup; the readdir future
/// runs without it. tmpfs readdir locks the directory inode and clones
/// names + kinds — independent of the FDS path.
pub async fn readdir(path: &str) -> Result<Vec<VfsDirent>, VfsError> {
    let parts = path::split(path)?;
    let (idx, sub) = resolve(&parts)?;
    let mounts = MOUNTS.lock();
    let fs = &mounts[idx].1;
    let result = fs.readdir(&sub).await;
    drop(mounts);
    result
}

/// Get file metadata (kind + size) without opening the file. Suitable for
/// `ls`/`stat`-style callers that need the kind/size enum without paying
/// the FD-table allocation cost. Like readdir, MOUNTS lock is dropped
/// after lookup; the inner await runs without it.
pub async fn stat(path: &str) -> Result<VfsStat, VfsError> {
    let parts = path::split(path)?;
    let (idx, sub) = resolve(&parts)?;
    let mounts = MOUNTS.lock();
    let fs = &mounts[idx].1;
    let result = fs.stat(&sub).await;
    drop(mounts);
    result
}
