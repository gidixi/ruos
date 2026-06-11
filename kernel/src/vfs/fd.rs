use alloc::vec::Vec;
use spin::Mutex;
use crate::vfs::error::VfsError;
use crate::vfs::file::{Fd, FileImpl};

pub(crate) struct FdEntry { pub file: FileImpl }

/// One global-fd slot. Three states, NOT an Option, because the take-and-
/// restore pattern (`with_fd_take`) must keep the slot RESERVED while an
/// async op is in flight: with a plain `None` an `allocate()` racing the op
/// (e.g. a component app opening its tty while the console shell sits in a
/// blocking stdin read) reuses the number, and on restore the old owner's
/// file is dropped — both owners end up silently cross-wired to the wrong
/// file (rtop-kill debug, 2026-06-11).
pub(crate) enum Slot {
    /// Reusable.
    Free,
    /// FdEntry temporarily taken by an in-flight `with_fd_take` op; the fd
    /// number stays reserved. `close()` during the op → Free (the restore
    /// then drops the entry, matching close-during-I/O semantics).
    InFlight,
    Open(FdEntry),
}

pub(crate) static FDS: Mutex<Vec<Slot>> = Mutex::new(Vec::new());

/// Insert `file` into the first Free slot (or push). Returns the Fd.
/// InFlight slots are NOT free — their fd number is still owned.
pub fn allocate(file: FileImpl) -> Fd {
    let mut t = FDS.lock();
    for (i, slot) in t.iter_mut().enumerate() {
        if matches!(slot, Slot::Free) {
            *slot = Slot::Open(FdEntry { file });
            return i as Fd;
        }
    }
    t.push(Slot::Open(FdEntry { file }));
    (t.len() - 1) as Fd
}

pub fn close(fd: Fd) -> Result<(), VfsError> {
    let mut t = FDS.lock();
    let slot = t.get_mut(fd as usize).ok_or(VfsError::BadFd)?;
    match slot {
        Slot::Free => Err(VfsError::BadFd),
        // Close during an in-flight op: free the number; the op's restore
        // finds Free and drops the entry (old close-during-I/O semantics).
        Slot::InFlight | Slot::Open(_) => { *slot = Slot::Free; Ok(()) }
    }
}

/// Take the entry out for an async op, leaving the slot RESERVED (InFlight).
pub(crate) fn take(fd: Fd) -> Result<FdEntry, VfsError> {
    let mut t = FDS.lock();
    let slot = t.get_mut(fd as usize).ok_or(VfsError::BadFd)?;
    match core::mem::replace(slot, Slot::InFlight) {
        Slot::Open(e) => Ok(e),
        // Free → bad fd; InFlight → a second concurrent op on the same fd
        // (unsupported — one op per fd at a time). Put the state back.
        other => { *slot = other_state(other); Err(VfsError::BadFd) }
    }
}

fn other_state(s: Slot) -> Slot {
    match s { Slot::Free => Slot::Free, _ => Slot::InFlight }
}

/// Put the entry back after the op. If the slot was closed meanwhile (Free),
/// drop the entry — the close won.
pub(crate) fn restore(fd: Fd, entry: FdEntry) {
    let mut t = FDS.lock();
    if let Some(slot) = t.get_mut(fd as usize) {
        if matches!(slot, Slot::InFlight) {
            *slot = Slot::Open(entry);
        }
        // Free → closed during the op → drop entry. Open → impossible
        // (allocate skips InFlight), keep the current owner.
    }
}

/// If `fd` is backed by a PTY slave, return its pair index. Used so exec'd
/// children / pipeline stages can inherit the caller's terminal (its PTY)
/// instead of defaulting to `/dev/pts/0`.
pub fn pts_index(fd: Fd) -> Option<usize> {
    let t = FDS.lock();
    match t.get(fd as usize) {
        Some(Slot::Open(e)) => match &e.file {
            FileImpl::PtySlave(f) => Some(f.idx),
            _ => None,
        },
        _ => None,
    }
}
