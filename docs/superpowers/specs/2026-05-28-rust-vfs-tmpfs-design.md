# Rust VFS + tmpfs + Device Files — Design Spec

**Date:** 2026-05-28
**Milestone:** Step 7 of the Rust OS roadmap (`docs/superpowers/roadmap-rust-os.md`).
**Status:** Approved design, ready for implementation planning.

## Context

The kernel now has a real paging stack (Step 6: frame allocator + Mapper) and
the heap (Step 4: talc). The next subsystem the WASM userland needs is a
file system: WASI Preview 1's `fd_*` and `path_*` host functions (Step 10)
sit directly on top of a VFS. Step 11's shell needs a path namespace to find
`/bin/foo.wasm`. Step 12's PTY and Step 15's SSH session need file-like
endpoints. Step 7 builds the minimum surface that unblocks all of these.

The post-pivot roadmap forbids per-process address spaces and Linux ABI, so
this VFS has no notion of "user" or "process" yet — there is one global file
descriptor table. When WASM modules acquire instance identity in Step 10,
the FD table grows a `(instance, fd)` index but the kernel-side API stays
identical.

## Goals

- A `FileSystem` trait (async, AFIT) implemented by `Tmpfs`. Static dispatch
  via `FsImpl` enum.
- A `File` trait (async, AFIT) implemented by `TmpfsFile`, `ConsoleFile`,
  `NullFile`, `ZeroFile`. Static dispatch via `FileImpl` enum.
- A `VfsError` enum with `Display`, mapped to WASI errno at the host-function
  boundary in Step 10.
- A global numeric FD table (`Fd = u32`) compatible with WASI semantics.
- One mount: tmpfs at `/`. Populated at boot with `/dev`, `/dev/console`,
  `/dev/null`, `/dev/zero`, and an empty `/tmp` directory.
- A `block_on` helper (no executor yet — Step 9) that drives a single future
  to completion via a noop-waker poll loop. Tmpfs and device futures all
  resolve on first poll.
- A boot-time smoke test in `kmain`: open `/dev/null` and write; create
  `/tmp/x`, write `"abc"`, seek to 0, read back, log result.
- `make run-test` keeps asserting `ruos: ticks=` (Step 5 line). The new
  serial lines (`ruos: vfs init ok mounts=1`, `ruos: vfs smoke ok n=3 buf=[abc]`)
  are observable but not the asserted string.

## Non-goals (YAGNI)

- No `/dev/random` (deferred to Step 14 — CSPRNG seeded from RDRAND).
- No FAT, no block device, no on-disk persistence (Step 7 is tmpfs only).
- No `readdir` (Step 11 shell `ls` will add it).
- No POSIX permissions (uid/gid/mode), symlinks, hard links, mmap.
- No multi-mount logic; the mount table holds exactly one entry at `/`.
- No per-process FD tables — a single global table.
- No real executor — `block_on` is a single-poll synchronizer until Step 9.

## Architecture

```
                 kmain
                   |
                   v  block_on(async { open/write/read })
   +-----------------------------------------------+
   | vfs::mod.rs   pub API (async fn open/...)     |
   +-----------------------------------------------+
        |                          |
        v                          v
   MOUNTS table             FDS table (global)
   (longest-prefix          (Vec<Option<FdEntry>>)
    match → FsImpl)
        |                          |
        v                          v
   enum FsImpl              enum FileImpl
   (Tmpfs)                  (TmpfsFile | Console | Null | Zero)
        |                          |
        v                          v
   tmpfs.rs                 devices.rs
   Tree of Arc<Mutex<TmpInode>>
```

## Module layout

```
kernel/src/vfs/
  mod.rs         # pub API: init, mount, open, close, read, write, seek
                 # MOUNTS static + dispatch
  error.rs       # VfsError enum + Display
  path.rs        # split + canonicalize (alloc::String / Vec<&str>)
  fs.rs          # trait FileSystem (AFIT) + enum FsImpl
  file.rs        # trait File (AFIT) + enum FileImpl + OpenFlags + Whence
  fd.rs          # global FD table + allocate / get / close
  tmpfs.rs       # struct Tmpfs + TmpInode + TmpfsFile
  devices.rs     # ConsoleFile / NullFile / ZeroFile
  block_on.rs    # noop_waker single-poll driver
```

The new sub-tree lives next to `kernel/src/memory/`, mirroring the layout
the project has already settled on for subsystems with multiple files.

## Components

### `vfs::error`

```rust
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
```
`impl Display for VfsError` returns short kebab-case strings
(`"not found"`, `"bad fd"`, etc.) so they're grep-friendly in serial logs and
will translate cleanly to WASI errno codes in Step 10.

### `vfs::path`

`pub fn split(path: &str) -> Result<Vec<&str>, VfsError>` — rejects empty
path, requires leading `/`, drops trailing `/`, splits on `/`, rejects
empty components and `.`/`..` (we don't support relative paths yet).

### `vfs::fs`

```rust
pub trait FileSystem {
    async fn open(&self, path: &[&str], flags: OpenFlags) -> Result<FileImpl, VfsError>;
    async fn create(&self, path: &[&str]) -> Result<(), VfsError>;
    async fn unlink(&self, path: &[&str]) -> Result<(), VfsError>;
}

pub enum FsImpl { Tmpfs(Tmpfs) }

impl FsImpl {
    pub async fn open(&self, path: &[&str], flags: OpenFlags) -> Result<FileImpl, VfsError> {
        match self { FsImpl::Tmpfs(t) => t.open(path, flags).await }
    }
    // create, unlink: identical pattern
}
```

`path` is the already-split slice from `path::split`, so each fs
implementation works on canonicalized components, not raw strings.

### `vfs::file`

```rust
pub type Fd = u32;

bitflags::bitflags! {
    pub struct OpenFlags: u32 {
        const READ     = 1 << 0;
        const WRITE    = 1 << 1;
        const CREATE   = 1 << 2;
        const TRUNCATE = 1 << 3;
    }
}

pub enum Whence { Set, Cur, End }

pub trait File {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, VfsError>;
    async fn write(&mut self, buf: &[u8]) -> Result<usize, VfsError>;
    async fn seek(&mut self, off: i64, whence: Whence) -> Result<u64, VfsError>;
}

pub enum FileImpl {
    Tmp(TmpfsFile),
    Console(ConsoleFile),
    Null(NullFile),
    Zero(ZeroFile),
}
// match-dispatched read/write/seek on FileImpl
```

### `vfs::fd`

```rust
struct FdEntry { file: FileImpl }
static FDS: spin::Mutex<Vec<Option<FdEntry>>> = spin::Mutex::new(Vec::new());

pub fn allocate(file: FileImpl) -> Fd;             // first free slot, else push
pub fn close(fd: Fd) -> Result<(), VfsError>;
// internal: with_file<F: FnOnce(&mut FileImpl)->R>(fd, f) for short-lived borrows
```

`Fd = 0/1/2` are NOT pre-reserved as stdin/stdout/stderr at this stage; the
WASM runtime in Step 10 will set up its own per-instance "preopens". The
shell (Step 11) will follow the same pattern.

### `vfs::tmpfs`

```rust
enum TmpKind { Dir, Reg }

struct TmpInode {
    kind: TmpKind,
    children: BTreeMap<String, Arc<Mutex<TmpInode>>>, // Dir-only
    content:  Vec<u8>,                                // Reg-only
}

pub struct Tmpfs { root: Arc<Mutex<TmpInode>> }
pub struct TmpfsFile { node: Arc<Mutex<TmpInode>>, pos: u64 }
```

Path lookup walks the tree holding each inode's mutex only briefly. The
write/read/seek implementations on `TmpfsFile` lock the node mutex, mutate
`content` or read it, update `pos`, release.

`mkdir` and `create` add a child to a parent's `children` map. `unlink`
removes one. `Tmpfs::open` walks to the parent, optionally creates the
target if `CREATE` is set, then constructs a `TmpfsFile { node, pos: 0 }`
and wraps it as `FileImpl::Tmp(...)`.

### `vfs::devices`

- `ConsoleFile`:
  - `read`: returns `Ok(0)` (EOF stub — Step 11 shell will swap in keyboard
    input).
  - `write(buf)`: streams every byte to `crate::serial::SERIAL` (lock
    briefly), returns `Ok(buf.len())`.
  - `seek`: returns `Err(VfsError::NotPermitted)`.
- `NullFile`: `read` → `Ok(0)`; `write` → `Ok(buf.len())`; `seek` → 0.
- `ZeroFile`: `read(buf)` → fills with `0u8`, `Ok(buf.len())`; `write` →
  `Ok(buf.len())`; `seek` → 0.

### `vfs::block_on`

```rust
pub fn block_on<F: Future>(mut fut: F) -> F::Output {
    use core::task::{Context, Poll, Waker, RawWaker, RawWakerVTable};
    use core::pin::Pin;

    static V: RawWakerVTable = RawWakerVTable::new(
        |_| RawWaker::new(core::ptr::null(), &V),
        |_| {}, |_| {}, |_| {},
    );
    let raw = RawWaker::new(core::ptr::null(), &V);
    // SAFETY: V is correctly constructed for a no-op waker.
    let waker = unsafe { Waker::from_raw(raw) };
    let mut cx = Context::from_waker(&waker);

    // SAFETY: fut is owned by us, never moved while pinned.
    let mut fut = unsafe { Pin::new_unchecked(&mut fut) };
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending  => continue, // not expected for tmpfs/devices
        }
    }
}
```

Once `embassy-executor` lands in Step 9, `block_on` either disappears
(callers move into async tasks) or stays as a debug helper.

### `vfs::mod`

```rust
static MOUNTS: spin::Mutex<Vec<(String, FsImpl)>> = spin::Mutex::new(Vec::new());

pub fn init() -> Result<(), VfsError> {
    // build empty Tmpfs, mount /, mkdir /dev, /tmp, create device files.
    // returns Ok with mount count for kmain log.
}
pub fn mount(prefix: &str, fs: FsImpl) -> Result<(), VfsError>;

pub async fn open(path: &str, flags: OpenFlags) -> Result<Fd, VfsError>;
pub async fn close(fd: Fd) -> Result<(), VfsError>;
pub async fn read(fd: Fd, buf: &mut [u8]) -> Result<usize, VfsError>;
pub async fn write(fd: Fd, buf: &[u8]) -> Result<usize, VfsError>;
pub async fn seek(fd: Fd, off: i64, whence: Whence) -> Result<u64, VfsError>;
```

`open` does: `path::split(path)?` → `MOUNTS.lock()` find longest-prefix
match → strip prefix → `fs.open(remaining_components, flags).await` →
`fd::allocate(file)` → `Ok(fd)`.

`read`/`write`/`seek` locks `FDS`, takes `&mut FileImpl` out of the slot for
the duration of the call (move-and-restore to avoid holding the table lock
across `.await`), drives the future, restores.

## Boot sequence (kmain additions)

Insert AFTER `keyboard::init(...)` and AFTER `sti` (so `kprintln!` from
devices works through the normal serial-IRQ-safe path), and BEFORE the
`while timer::ticks() < 10` busy wait:

```rust
match memory::vfs::init() {   // actually crate::vfs::init()
    Ok(()) => kprintln!("ruos: vfs init ok mounts=1"),
    Err(e) => {
        kprintln!("ruos: vfs init fail: {}", e);
        hcf();
    }
}

let smoke = vfs::block_on(async {
    use vfs::file::{OpenFlags, Whence};
    // /dev/null write
    let fd = vfs::open("/dev/null", OpenFlags::WRITE).await?;
    vfs::write(fd, b"hello").await?;
    vfs::close(fd).await?;
    // /tmp/x create+write+seek+read
    let fd = vfs::open("/tmp/x",
        OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::READ).await?;
    vfs::write(fd, b"abc").await?;
    vfs::seek(fd, 0, Whence::Set).await?;
    let mut buf = [0u8; 8];
    let n = vfs::read(fd, &mut buf).await?;
    vfs::close(fd).await?;
    Ok::<(usize, [u8; 8]), vfs::error::VfsError>((n, buf))
});
match smoke {
    Ok((n, buf)) => kprintln!(
        "ruos: vfs smoke ok n={} buf=[{}]",
        n,
        core::str::from_utf8(&buf[..n]).unwrap_or("?")
    ),
    Err(e) => {
        kprintln!("ruos: vfs smoke fail: {}", e);
        hcf();
    }
}
```

## Data flow

```
boot:
  vfs::init() → MOUNTS = [("/", FsImpl::Tmpfs(empty))]
              → Tmpfs creates /dev, /tmp directories
              → Tmpfs creates /dev/console, /dev/null, /dev/zero device files
                (these are "regular" inodes whose open() returns FileImpl::{Console,Null,Zero})

open("/dev/null", WRITE):
  path::split → ["dev", "null"]
  MOUNTS longest-prefix → FsImpl::Tmpfs at /, strip "" prefix
  Tmpfs.open(["dev", "null"], WRITE).await
    → walk root.children["dev"].children["null"]
    → device dispatch: return FileImpl::Null(NullFile)
  fd::allocate(FileImpl::Null) → Fd::new(2)
  return 2

write(2, b"hello"):
  FDS[2].file → take &mut → FileImpl::Null → returns Ok(5)
```

## Errors

Every failure path logs via `kprintln!` and either returns the error up to
the caller (open/read/write) or halts the kernel (init/smoke failure):

- `ruos: vfs init fail: <VfsError>` + `hcf()` if `init` returns Err.
- `ruos: vfs smoke fail: <VfsError>` + `hcf()` if the smoke-test future
  returns Err.
- Application-level errors (caller closes a closed fd, reads from non-readable
  fd, etc.) bubble up as `VfsError`; Step 10 will translate them to WASI
  errno.

## Testing

- **Automated (`make run-test`)** keeps asserting `ruos: ticks=`. Reaching it
  now also implies VFS init + smoke succeeded — the smoke `hcf()` on failure
  would prevent the timer-tick line from ever appearing.
- **Manual:** the boot log shows
  - `ruos: vfs init ok mounts=1`
  - `ruos: vfs smoke ok n=3 buf=[abc]`
  Inspecting `build/serial.log` confirms both.
- **Negative paths** (BadFd, NotFound, InvalidPath) are exercised by code
  review of the error branches plus targeted unit-test-style additions in
  the smoke routine if desired (out of scope for this milestone).

## Decomposition into tasks

1. **Core types + traits + path + block_on** — `vfs/{mod.rs scheletro,
   error.rs, path.rs, fs.rs, file.rs, fd.rs, block_on.rs}`. Compiles green;
   no kmain wiring yet. No new serial lines.
2. **tmpfs + devices + init populating** — `vfs/{tmpfs.rs, devices.rs}` +
   complete `vfs::init()`. kmain calls `vfs::init()` after `sti`, logs
   `ruos: vfs init ok mounts=1`. No smoke test yet.
3. **API open/close/read/write/seek + smoke test** — complete `vfs::mod`
   API dispatch via FDS + MOUNTS, run boot-time smoke test, log
   `ruos: vfs smoke ok n=3 buf=[abc]`. `TEST_PASS` preserved end-to-end.

## Open items for the implementation plan

- Whether `OpenFlags` uses the `bitflags` crate or a hand-rolled `u32`
  constants module (the plan should pick one and stick with it).
- Whether `Mutex<TmpInode>` is `spin::Mutex` (current convention) or
  something with try-lock semantics for nested-path locking races.
- Final shape of `with_file` helper in `fd.rs` (move-out vs.
  re-entrant-aware borrowing).
