use alloc::vec::Vec;
use spin::Mutex;
use crate::vfs::error::VfsError;
use crate::vfs::file::{Fd, FileImpl};

pub(crate) struct FdEntry { pub file: FileImpl }

pub(crate) static FDS: Mutex<Vec<Option<FdEntry>>> = Mutex::new(Vec::new());

/// Insert `file` into the first free slot (or push). Returns the Fd.
pub fn allocate(file: FileImpl) -> Fd {
    let mut t = FDS.lock();
    for (i, slot) in t.iter_mut().enumerate() {
        if slot.is_none() {
            *slot = Some(FdEntry { file });
            return i as Fd;
        }
    }
    t.push(Some(FdEntry { file }));
    (t.len() - 1) as Fd
}

pub fn close(fd: Fd) -> Result<(), VfsError> {
    let mut t = FDS.lock();
    let slot = t.get_mut(fd as usize).ok_or(VfsError::BadFd)?;
    if slot.is_none() { return Err(VfsError::BadFd); }
    *slot = None;
    Ok(())
}

/// If `fd` is backed by a PTY slave, return its pair index. Used so exec'd
/// children / pipeline stages can inherit the caller's terminal (its PTY)
/// instead of defaulting to `/dev/pts/0`.
pub fn pts_index(fd: Fd) -> Option<usize> {
    let t = FDS.lock();
    match t.get(fd as usize).and_then(|s| s.as_ref()) {
        Some(e) => match &e.file {
            FileImpl::PtySlave(f) => Some(f.idx),
            _ => None,
        },
        None => None,
    }
}
