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
