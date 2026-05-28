# Rust VFS + tmpfs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an async VFS API with a tmpfs in-RAM filesystem mounted at `/`, device files (`/dev/console`, `/dev/null`, `/dev/zero`), and a numeric global FD table; boot-time smoke test writes/reads `/tmp/x` and `/dev/null`.

**Architecture:** New `kernel/src/vfs/` module tree. Async traits via native AFIT (Rust 1.75+), enum dispatch (`FsImpl` / `FileImpl`) for static no-alloc-per-call polymorphism. A noop-waker `block_on` lets `kmain` drive futures synchronously until Step 9's executor lands; tmpfs and device futures always resolve on first poll. The FD table is a single global `Vec<Option<FdEntry>>`; per-process tables arrive with the WASM runtime in Step 10.

**Tech Stack:** Rust nightly `nightly-2026-05-26`, existing `spin`/`alloc`/`x86_64`/`limine`/`talc`/`uart_16550`. New dep: `bitflags = "2"` for `OpenFlags`. WSL Ubuntu host.

---

## Key facts

- All build/run via **WSL Ubuntu** as root, cargo env sourced:
  ```
  wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && <cmd>'
  ```
  Edit on Windows paths. Git in normal shell. Branch `feature/vfs-tmpfs`. Do not push, do not skip hooks.
- **Spec:** `docs/superpowers/specs/2026-05-28-rust-vfs-tmpfs-design.md`.
- `kmain` currently ends with: heap → gdt → idt → int3 → pic → acpi → frames → mapper → smoke → lapic → ioapic → timer → keyboard → sti → busy-wait `ticks() < 10` → log `ruos: ticks=` → halt. New VFS init + smoke insert AFTER `sti` and BEFORE the busy-wait, so `kprintln!` from the smoke test runs with interrupts enabled (matches how the existing busy-wait already operates).
- The `kprintln!` macro is already deadlock-safe (wraps `without_interrupts`).
- TEST_PASS Makefile assert stays `ruos: ticks=`. New VFS lines (`ruos: vfs init ok mounts=1`, `ruos: vfs smoke ok n=3 buf=[abc]`) are observable in `build/serial.log` but not asserted.

## File structure (target)

```
kernel/src/vfs/
  mod.rs        # API: init, mount, open, close, read, write, seek; MOUNTS + dispatch
  error.rs      # VfsError enum + Display
  path.rs       # split + validation; Vec<&str> output
  fs.rs         # trait FileSystem (AFIT) + FsImpl enum
  file.rs       # trait File (AFIT) + FileImpl enum + OpenFlags + Whence + Fd
  fd.rs         # global FD table + allocate/close/with_file
  tmpfs.rs      # Tmpfs + TmpInode + TmpfsFile
  devices.rs    # ConsoleFile + NullFile + ZeroFile
  block_on.rs   # noop_waker single-poll driver
```

---

## Task 1: Core types + traits + path + block_on (no runtime change)

**Files:**
- Modify: `kernel/Cargo.toml`
- Create: `kernel/src/vfs/mod.rs`
- Create: `kernel/src/vfs/error.rs`
- Create: `kernel/src/vfs/path.rs`
- Create: `kernel/src/vfs/file.rs`
- Create: `kernel/src/vfs/fs.rs`
- Create: `kernel/src/vfs/fd.rs`
- Create: `kernel/src/vfs/block_on.rs`
- Modify: `kernel/src/main.rs`
- Create: `CHANGELOG/40-26-05-28-vfs-core-types.md`

- [ ] **Step 1: Add `bitflags` dep**

In `kernel/Cargo.toml` `[dependencies]`, add (alongside the others):
```toml
bitflags = "2"
```

- [ ] **Step 2: Create `kernel/src/vfs/error.rs`**
```rust
use core::fmt;

#[derive(Debug, Copy, Clone)]
pub enum VfsError {
    NotFound,
    AlreadyExists,
    NotDirectory,
    IsDirectory,
    BadFd,
    NotPermitted,
    InvalidPath,
    Closed,
    NoSpace,
    Other,
}

impl fmt::Display for VfsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            VfsError::NotFound      => "not found",
            VfsError::AlreadyExists => "already exists",
            VfsError::NotDirectory  => "not directory",
            VfsError::IsDirectory   => "is directory",
            VfsError::BadFd         => "bad fd",
            VfsError::NotPermitted  => "not permitted",
            VfsError::InvalidPath   => "invalid path",
            VfsError::Closed        => "closed",
            VfsError::NoSpace       => "no space",
            VfsError::Other         => "other",
        };
        f.write_str(s)
    }
}
```

- [ ] **Step 3: Create `kernel/src/vfs/path.rs`**
```rust
use alloc::vec::Vec;
use crate::vfs::error::VfsError;

/// Split an absolute path into canonical components. Rejects empty paths,
/// missing leading '/', empty components, '.', and '..'.
///
/// `"/"`         -> `Ok(vec![])`
/// `"/dev"`      -> `Ok(vec!["dev"])`
/// `"/dev/null"` -> `Ok(vec!["dev", "null"])`
pub fn split(path: &str) -> Result<Vec<&str>, VfsError> {
    let rest = path.strip_prefix('/').ok_or(VfsError::InvalidPath)?;
    let trimmed = rest.trim_end_matches('/');
    if trimmed.is_empty() { return Ok(Vec::new()); }
    let mut parts = Vec::new();
    for c in trimmed.split('/') {
        if c.is_empty() || c == "." || c == ".." {
            return Err(VfsError::InvalidPath);
        }
        parts.push(c);
    }
    Ok(parts)
}
```

- [ ] **Step 4: Create `kernel/src/vfs/file.rs`** (trait + types; `FileImpl` enum dispatches stubs that will get bodies in Task 2)
```rust
use crate::vfs::error::VfsError;

pub type Fd = u32;

bitflags::bitflags! {
    #[derive(Debug, Copy, Clone)]
    pub struct OpenFlags: u32 {
        const READ     = 1 << 0;
        const WRITE    = 1 << 1;
        const CREATE   = 1 << 2;
        const TRUNCATE = 1 << 3;
    }
}

#[derive(Debug, Copy, Clone)]
pub enum Whence { Set, Cur, End }

pub trait File {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, VfsError>;
    async fn write(&mut self, buf: &[u8]) -> Result<usize, VfsError>;
    async fn seek(&mut self, off: i64, whence: Whence) -> Result<u64, VfsError>;
}

// Concrete file types are introduced in Task 2 (tmpfs + devices).
// FileImpl variants below will be filled in then; for Task 1 we declare an
// empty placeholder so other modules can name the type. The variants are
// constructed only by Task 2 code, so no real values flow through yet.
pub enum FileImpl {
    Placeholder,
}

impl FileImpl {
    pub async fn read(&mut self, _buf: &mut [u8]) -> Result<usize, VfsError> {
        Err(VfsError::Other)
    }
    pub async fn write(&mut self, _buf: &[u8]) -> Result<usize, VfsError> {
        Err(VfsError::Other)
    }
    pub async fn seek(&mut self, _off: i64, _whence: Whence) -> Result<u64, VfsError> {
        Err(VfsError::Other)
    }
}
```

(Task 2 replaces the `FileImpl` body with the real four variants. The
placeholder lets Task 1 build green without forward references.)

- [ ] **Step 5: Create `kernel/src/vfs/fs.rs`** (trait + `FsImpl` placeholder)
```rust
use crate::vfs::error::VfsError;
use crate::vfs::file::{FileImpl, OpenFlags};

pub trait FileSystem {
    async fn open(&self, path: &[&str], flags: OpenFlags) -> Result<FileImpl, VfsError>;
    async fn create(&self, path: &[&str]) -> Result<(), VfsError>;
    async fn unlink(&self, path: &[&str]) -> Result<(), VfsError>;
}

// Concrete filesystems are introduced in Task 2 (Tmpfs). Same placeholder
// pattern as FileImpl.
pub enum FsImpl {
    Placeholder,
}

impl FsImpl {
    pub async fn open(&self, _path: &[&str], _flags: OpenFlags) -> Result<FileImpl, VfsError> {
        Err(VfsError::Other)
    }
    pub async fn create(&self, _path: &[&str]) -> Result<(), VfsError> {
        Err(VfsError::Other)
    }
    pub async fn unlink(&self, _path: &[&str]) -> Result<(), VfsError> {
        Err(VfsError::Other)
    }
}
```

- [ ] **Step 6: Create `kernel/src/vfs/fd.rs`**
```rust
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
```

- [ ] **Step 7: Create `kernel/src/vfs/block_on.rs`**
```rust
//! Single-poll synchronous driver for VFS futures. Tmpfs and device futures
//! never yield Pending, so a noop waker plus a poll loop completes them on
//! the first poll. When `embassy-executor` lands in Step 9, this helper
//! becomes obsolete or stays as a debug-only escape hatch.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

static VTABLE: RawWakerVTable = RawWakerVTable::new(
    |_| RawWaker::new(core::ptr::null(), &VTABLE),
    |_| {},
    |_| {},
    |_| {},
);

pub fn block_on<F: Future>(mut fut: F) -> F::Output {
    let raw = RawWaker::new(core::ptr::null(), &VTABLE);
    // SAFETY: VTABLE has all four fn pointers and the data pointer is unused.
    let waker = unsafe { Waker::from_raw(raw) };
    let mut cx = Context::from_waker(&waker);
    // SAFETY: `fut` is owned here and never moved while pinned.
    let mut fut = unsafe { Pin::new_unchecked(&mut fut) };
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending  => continue,
        }
    }
}
```

- [ ] **Step 8: Create `kernel/src/vfs/mod.rs`** (skeleton with re-exports; API stubs filled in Task 3)
```rust
//! Async VFS API + tmpfs in-RAM filesystem + device files.

pub mod error;
pub mod path;
pub mod file;
pub mod fs;
pub mod fd;
pub mod block_on;

pub use block_on::block_on;
pub use error::VfsError;
pub use file::{Fd, OpenFlags, Whence};

// Real API (open/close/read/write/seek/init/mount) lands in Task 3.
```

- [ ] **Step 9: Wire `mod vfs;` into `kernel/src/main.rs`**

Add `mod vfs;` next to the other top-level `mod` declarations (alongside `mod memory;`, `mod gdt;`, etc.). No runtime change yet.

- [ ] **Step 10: Build**
```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -5'
```
Expected: `TEST_PASS` (no behavioral change — all VFS symbols are inert placeholders). `cargo build` should emit `dead_code` warnings for the placeholder variants and the unused `FDS` static; that is fine until Task 2/3.

- [ ] **Step 11: Changelog**

Create `CHANGELOG/40-26-05-28-vfs-core-types.md`:
```markdown
# 40 — VFS core types + traits + path + block_on (Step 7 Task 1)

**Data:** 2026-05-28

## Cosa
- Aggiunta dep `bitflags = "2"`.
- Nuovo modulo `kernel/src/vfs/` con:
  - `error.rs` — `VfsError` enum + `Display`.
  - `path.rs` — `split(path)` (Vec<&str>, rifiuta '.', '..', componenti vuoti).
  - `file.rs` — trait `File` (async AFIT) + `FileImpl` placeholder + `OpenFlags`
    (`bitflags`) + `Whence` + `Fd = u32`.
  - `fs.rs` — trait `FileSystem` (async AFIT) + `FsImpl` placeholder.
  - `fd.rs` — `FDS: spin::Mutex<Vec<Option<FdEntry>>>` + `allocate/close`.
  - `block_on.rs` — noop_waker single-poll driver per chiamare async dal kmain
    finché Step 9 non porta `embassy-executor`.
  - `mod.rs` — re-exports.
- `main.rs`: `mod vfs;`.
- Nessun cambio runtime; placeholder variants + stati inerti finché Task 2/3
  riempiono.

## Perché
Primo pezzo dello Step 7: API surface + skeleton senza coinvolgere kmain.

## File toccati
- kernel/Cargo.toml, kernel/Cargo.lock
- kernel/src/vfs/* (nuovi)
- kernel/src/main.rs
- CHANGELOG/40-26-05-28-vfs-core-types.md
```

- [ ] **Step 12: Commit**
```bash
git add kernel/Cargo.toml kernel/Cargo.lock kernel/src/vfs kernel/src/main.rs \
        CHANGELOG/40-26-05-28-vfs-core-types.md
git commit -m "feat(rust): VFS core types, traits, path parser, block_on driver"
```

---

## Task 2: tmpfs + devices + boot init populating

**Files:**
- Modify: `kernel/src/vfs/file.rs` (`FileImpl` variants)
- Modify: `kernel/src/vfs/fs.rs` (`FsImpl::Tmpfs` variant)
- Create: `kernel/src/vfs/tmpfs.rs`
- Create: `kernel/src/vfs/devices.rs`
- Modify: `kernel/src/vfs/mod.rs` (`init` + `mount` + `MOUNTS`)
- Modify: `kernel/src/main.rs`
- Create: `CHANGELOG/41-26-05-28-vfs-tmpfs-devices.md`

- [ ] **Step 1: Replace placeholder `FileImpl` in `kernel/src/vfs/file.rs`**

Replace the placeholder `enum FileImpl { Placeholder }` and its `impl` block with:
```rust
use crate::vfs::tmpfs::TmpfsFile;
use crate::vfs::devices::{ConsoleFile, NullFile, ZeroFile};

pub enum FileImpl {
    Tmp(TmpfsFile),
    Console(ConsoleFile),
    Null(NullFile),
    Zero(ZeroFile),
}

impl FileImpl {
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize, VfsError> {
        match self {
            FileImpl::Tmp(f)     => f.read(buf).await,
            FileImpl::Console(f) => f.read(buf).await,
            FileImpl::Null(f)    => f.read(buf).await,
            FileImpl::Zero(f)    => f.read(buf).await,
        }
    }
    pub async fn write(&mut self, buf: &[u8]) -> Result<usize, VfsError> {
        match self {
            FileImpl::Tmp(f)     => f.write(buf).await,
            FileImpl::Console(f) => f.write(buf).await,
            FileImpl::Null(f)    => f.write(buf).await,
            FileImpl::Zero(f)    => f.write(buf).await,
        }
    }
    pub async fn seek(&mut self, off: i64, whence: Whence) -> Result<u64, VfsError> {
        match self {
            FileImpl::Tmp(f)     => f.seek(off, whence).await,
            FileImpl::Console(f) => f.seek(off, whence).await,
            FileImpl::Null(f)    => f.seek(off, whence).await,
            FileImpl::Zero(f)    => f.seek(off, whence).await,
        }
    }
}
```

Keep the `use crate::vfs::file::File;` brought into scope or qualify the
method calls inline. Since `File` is the trait whose methods we're calling,
`use crate::vfs::file::File as _;` at the top of `file.rs` brings it in.

- [ ] **Step 2: Create `kernel/src/vfs/devices.rs`**
```rust
//! Device files: console (serial), null, zero.

use crate::vfs::error::VfsError;
use crate::vfs::file::{File, Whence};
use core::fmt::Write as _;

pub struct ConsoleFile;
pub struct NullFile;
pub struct ZeroFile;

impl File for ConsoleFile {
    async fn read(&mut self, _buf: &mut [u8]) -> Result<usize, VfsError> {
        // EOF stub until Step 11 wires keyboard input into stdin.
        Ok(0)
    }
    async fn write(&mut self, buf: &[u8]) -> Result<usize, VfsError> {
        let mut serial = crate::serial::SERIAL.lock();
        for b in buf {
            // Write each byte as a 1-char str. Non-UTF-8 bytes print as '?'.
            let _ = serial.write_str(core::str::from_utf8(&[*b]).unwrap_or("?"));
        }
        Ok(buf.len())
    }
    async fn seek(&mut self, _off: i64, _w: Whence) -> Result<u64, VfsError> {
        Err(VfsError::NotPermitted)
    }
}

impl File for NullFile {
    async fn read(&mut self, _buf: &mut [u8]) -> Result<usize, VfsError> { Ok(0) }
    async fn write(&mut self, buf: &[u8]) -> Result<usize, VfsError> { Ok(buf.len()) }
    async fn seek(&mut self, _off: i64, _w: Whence) -> Result<u64, VfsError> { Ok(0) }
}

impl File for ZeroFile {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, VfsError> {
        for b in buf.iter_mut() { *b = 0; }
        Ok(buf.len())
    }
    async fn write(&mut self, buf: &[u8]) -> Result<usize, VfsError> { Ok(buf.len()) }
    async fn seek(&mut self, _off: i64, _w: Whence) -> Result<u64, VfsError> { Ok(0) }
}
```


- [ ] **Step 3: Create `kernel/src/vfs/tmpfs.rs`**
```rust
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
    fn new_device(kind: TmpKind) -> Self {
        Self { kind, children: BTreeMap::new(), content: Vec::new() }
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
```

- [ ] **Step 4: Replace placeholder `FsImpl` in `kernel/src/vfs/fs.rs`**

Replace `enum FsImpl { Placeholder }` and its `impl` with:
```rust
use crate::vfs::tmpfs::Tmpfs;

pub enum FsImpl {
    Tmpfs(Tmpfs),
}

impl FsImpl {
    pub async fn open(&self, path: &[&str], flags: OpenFlags) -> Result<FileImpl, VfsError> {
        match self { FsImpl::Tmpfs(t) => t.open(path, flags).await }
    }
    pub async fn create(&self, path: &[&str]) -> Result<(), VfsError> {
        match self { FsImpl::Tmpfs(t) => t.create(path).await }
    }
    pub async fn unlink(&self, path: &[&str]) -> Result<(), VfsError> {
        match self { FsImpl::Tmpfs(t) => t.unlink(path).await }
    }
}
```

- [ ] **Step 5: Add `MOUNTS` + `mount` + `init` to `kernel/src/vfs/mod.rs`**

Append to the existing `mod.rs` content:
```rust
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

use crate::vfs::error::VfsError;
use crate::vfs::fs::FsImpl;
use crate::vfs::tmpfs::{Tmpfs, TmpInode, TmpKind};

pub(crate) static MOUNTS: Mutex<Vec<(String, FsImpl)>> = Mutex::new(Vec::new());

pub fn mount(prefix: &str, fs: FsImpl) -> Result<(), VfsError> {
    if !prefix.starts_with('/') { return Err(VfsError::InvalidPath); }
    MOUNTS.lock().push((prefix.to_string(), fs));
    Ok(())
}

/// Build the in-RAM root tmpfs, mount it at `/`, populate /dev + /tmp.
pub fn init() -> Result<usize, VfsError> {
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
```

- [ ] **Step 6: Wire `vfs::init()` in `kmain`**

Edit `kernel/src/main.rs`. After the `x86_64::instructions::interrupts::enable();` (`sti`) and BEFORE the `while timer::ticks() < 10` busy-wait, add:
```rust
    match vfs::init() {
        Ok(n) => kprintln!("ruos: vfs init ok mounts={}", n),
        Err(e) => {
            kprintln!("ruos: vfs init fail: {}", e);
            hcf();
        }
    }
```

- [ ] **Step 7: Build and run**
```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -12'
```
Expected serial includes the new line right before `ruos: ticks=`:
```
ruos: vfs init ok mounts=1
ruos: ticks=N
```
and `TEST_PASS`. No regression.

- [ ] **Step 8: Changelog**
Create `CHANGELOG/41-26-05-28-vfs-tmpfs-devices.md`:
```markdown
# 41 — VFS tmpfs + devices + boot init (Step 7 Task 2)

**Data:** 2026-05-28

## Cosa
- `kernel/src/vfs/file.rs`: `FileImpl` placeholder rimpiazzato con
  `Tmp(TmpfsFile)`, `Console(ConsoleFile)`, `Null(NullFile)`, `Zero(ZeroFile)`;
  dispatch read/write/seek su tutte le variant.
- `kernel/src/vfs/fs.rs`: `FsImpl::Tmpfs(Tmpfs)`.
- `kernel/src/vfs/tmpfs.rs`: `Tmpfs` + `TmpInode` + `TmpfsFile`. Albero
  `Arc<Mutex<TmpInode>>`. Impl `FileSystem` (open con CREATE, create, unlink).
  Impl `File` per `TmpfsFile` (read/write/seek su Vec<u8>).
- `kernel/src/vfs/devices.rs`: `ConsoleFile` (write→SERIAL byte-per-byte,
  read=0, seek=NotPermitted), `NullFile`, `ZeroFile`.
- `kernel/src/vfs/mod.rs`: `MOUNTS` static + `mount(prefix, fs)` + `init()`
  che costruisce tmpfs, mkdir `/dev` + `/tmp`, crea `/dev/{console,null,zero}`,
  monta `/`. Ritorna numero di mount.
- `kmain`: dopo `sti`, chiama `vfs::init()` e logga
  `ruos: vfs init ok mounts=1`.

## Perché
Secondo pezzo dello Step 7: tmpfs + device files presenti e montati al boot.
API open/close/read/write/seek arriverà in Task 3.

## File toccati
- kernel/src/vfs/file.rs, fs.rs, tmpfs.rs (nuovo), devices.rs (nuovo), mod.rs
- kernel/src/main.rs
- CHANGELOG/41-26-05-28-vfs-tmpfs-devices.md
```

- [ ] **Step 9: Commit**
```bash
git add kernel/src/vfs kernel/src/main.rs CHANGELOG/41-26-05-28-vfs-tmpfs-devices.md
git commit -m "feat(rust): tmpfs + device files + boot mount at /"
```

---

## Task 3: API dispatch + boot-time smoke test

**Files:**
- Modify: `kernel/src/vfs/mod.rs`
- Modify: `kernel/src/main.rs`
- Create: `CHANGELOG/42-26-05-28-vfs-smoke.md`

- [ ] **Step 1: Add async API to `kernel/src/vfs/mod.rs`**

Append to `mod.rs` (after the `init` function):
```rust
use crate::vfs::file::{Fd, FileImpl, OpenFlags, Whence};
use crate::vfs::fd::{FDS, FdEntry, allocate as fd_allocate, close as fd_close};

/// Locate the FsImpl covering `abspath` and return the components below the
/// mount point. Longest-prefix match.
fn resolve<'a>(abspath: &'a [&'a str]) -> Result<(usize, alloc::vec::Vec<&'a str>), VfsError> {
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
    let mut t = FDS.lock();
    let slot = t.get_mut(fd as usize).ok_or(VfsError::BadFd)?
        .as_mut().ok_or(VfsError::BadFd)?;
    slot.file.write(buf).await
}

pub async fn seek(fd: Fd, off: i64, whence: Whence) -> Result<u64, VfsError> {
    let mut t = FDS.lock();
    let slot = t.get_mut(fd as usize).ok_or(VfsError::BadFd)?
        .as_mut().ok_or(VfsError::BadFd)?;
    slot.file.seek(off, whence).await
}
```

(The `resolve` function is a stub for the single-mount world. When multi-mount
arrives, replace its body with longest-prefix matching; the call sites stay
identical.)

- [ ] **Step 2: Wire the smoke test in `kmain`**

Edit `kernel/src/main.rs`. After the `ruos: vfs init ok mounts=1` log, add:
```rust
    let smoke = vfs::block_on(async {
        use vfs::{open, write, read, seek, close, OpenFlags, Whence};
        // /dev/null write
        let fd = open("/dev/null", OpenFlags::WRITE).await?;
        write(fd, b"hello").await?;
        close(fd).await?;
        // /tmp/x: create, write, seek to start, read back.
        let fd = open(
            "/tmp/x",
            OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::READ,
        ).await?;
        write(fd, b"abc").await?;
        seek(fd, 0, Whence::Set).await?;
        let mut buf = [0u8; 8];
        let n = read(fd, &mut buf).await?;
        close(fd).await?;
        Ok::<(usize, [u8; 8]), vfs::VfsError>((n, buf))
    });
    match smoke {
        Ok((n, buf)) => kprintln!(
            "ruos: vfs smoke ok n={} buf=[{}]",
            n,
            core::str::from_utf8(&buf[..n]).unwrap_or("?"),
        ),
        Err(e) => {
            kprintln!("ruos: vfs smoke fail: {}", e);
            hcf();
        }
    }
```

- [ ] **Step 3: Build and run**
```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -15'
```
Expected serial includes BOTH new lines plus `TEST_PASS`:
```
ruos: vfs init ok mounts=1
ruos: vfs smoke ok n=3 buf=[abc]
ruos: ticks=N
```

If `ruos: vfs smoke fail: <err>` appears, the message names the failing
`VfsError` variant — `not found` would mean the `/dev/null` path doesn't
resolve, `is directory` means the smoke opened the wrong inode kind, etc.

- [ ] **Step 4: Changelog**
Create `CHANGELOG/42-26-05-28-vfs-smoke.md`:
```markdown
# 42 — VFS API dispatch + boot smoke test (Step 7 Task 3)

**Data:** 2026-05-28

## Cosa
- `kernel/src/vfs/mod.rs`: API pubblica `open`/`close`/`read`/`write`/`seek`
  (async) dispatchata via `MOUNTS` (longest-prefix stub: 1 mount) + `FDS`
  table globale.
- `kmain`: smoke test al boot — open `/dev/null` + write; create `/tmp/x` +
  write `"abc"` + seek 0 + read back. Log
  `ruos: vfs smoke ok n=3 buf=[abc]`.
- `TEST_PASS` preservato (assertion `ruos: ticks=` invariata; le nuove righe
  vfs sono prerequisito implicito perché il fail halta prima del ticks).

## Perché
Chiude lo Step 7: VFS operativo end-to-end, pronto per Step 10 (WASI host
fn) e Step 11 (shell lookup).

## File toccati
- kernel/src/vfs/mod.rs
- kernel/src/main.rs
- CHANGELOG/42-26-05-28-vfs-smoke.md
```

- [ ] **Step 5: Commit**
```bash
git add kernel/src/vfs/mod.rs kernel/src/main.rs CHANGELOG/42-26-05-28-vfs-smoke.md
git commit -m "feat(rust): VFS open/close/read/write/seek + boot smoke test"
```

---

## Notes for the implementer

- **WSL + cargo env** for every build/run.
- **AFIT (async fn in trait)** is stable since Rust 1.75 — the pinned nightly
  (`nightly-2026-05-26`) supports it directly. No `async-trait` crate, no
  boxed futures.
- **`spin::Mutex` held across `.await`** is technically dangerous if the
  executor can suspend (Step 9). For Step 7, all `await` points are inside
  tmpfs/device futures that complete on first poll, so no suspension occurs
  and the lock is always released within the same poll. Comment on the
  `FDS.lock()`-across-await pattern in `vfs::read`/`write`/`seek` flags this
  for the Step 9 refactor.
- **`FileImpl` and `FsImpl`** are enums, not trait objects. Adding a new fs
  (FAT) or file type (PTY, socket) means adding a variant + a match arm. The
  variants `TmpKind::DevConsole`/`DevNull`/`DevZero` keep device files
  representable inside tmpfs's inode tree even though their open() returns a
  non-tmpfs `FileImpl` variant.
- **No `/dev/random` here.** Step 14 wires CSPRNG (ChaCha20 seeded from
  RDRAND) and adds the device.
