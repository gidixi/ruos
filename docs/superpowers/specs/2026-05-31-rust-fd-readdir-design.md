# `fd_readdir` — WASI directory enumeration: Design Spec + Implementation Plan

## Context

Today directory listing on ruos goes through a **custom** host function,
`ruos.readdir` (`kernel/src/wasm/host/proc.rs:193`), which the userspace tools
(`ls`, `find`, `du`, `grep -r`) call via `#[link(wasm_import_module = "ruos")]`
bindings with a bespoke 12-byte record layout. Standard Rust `std::fs::read_dir`
does **not** work: on `wasm32-wasip1` it lowers to the WASI Preview 1
`fd_readdir` syscall, which we do not export. The consequence is that any
off-the-shelf crate that walks directories (`walkdir`, `ignore`, `glob`,
`std::fs::read_dir` directly) fails to run even after recompiling to
`wasm32-wasip1`.

This is the first item of the broader "increase WASI/WASIX compatibility" work.
It is deliberately scoped small: it is low-risk, reuses enumeration logic we
already have, and unblocks `std`-native directory walking so future tools need
no custom bindings.

The blocker is structural, not just a missing export:

- `FdEntry` (`kernel/src/wasm/state.rs:20`) has only `StdoutConsole`,
  `Vfs(vfs::Fd)`, `Socket(usize)` — there is **no directory fd variant**.
- `path_open` (`kernel/src/wasm/host/path.rs:28`) with `O_DIRECTORY` currently
  does nothing special; `vfs::open` returns `IsDirectory` for directories, so an
  open of a directory fails. wasi-libc's `opendir`/`fdopendir` path therefore
  never gets a usable fd to call `fd_readdir` on.

So this work is: **model an open directory as a first-class fd**, make
`path_open(O_DIRECTORY)` return one, then implement `fd_readdir` against it,
reusing the existing async VFS enumeration.

## Goals

- Export `wasi_snapshot_preview1::fd_readdir` with correct Preview 1 ABI
  (24-byte `__wasi_dirent_t` + name bytes, cookie-based resumption, partial
  last-entry truncation).
- Make `path_open` with `O_DIRECTORY` succeed for real directories, returning a
  new `FdEntry::Dir` fd; return `ENOTDIR` (54) when the target is not a
  directory.
- Make `fd_fdstat_get` and `fd_close` handle `FdEntry::Dir`.
- Result: `std::fs::read_dir(path)` works from a plain `wasm32-wasip1` `std`
  binary, including `.`/`..` filtering and recursion via `walkdir`.

## Non-goals (YAGNI)

- **Do not** remove or change `ruos.readdir`. It stays for backward compat; the
  existing tools keep working unchanged. Migrating `ls`/`find`/`du`/`grep` to
  `std::fs::read_dir` is a *separate*, optional follow-up changelog.
- No real inode numbers. `d_ino` may be `0` (or a cheap stable hash of the
  path); `std::fs::read_dir` does not depend on it.
- No `fd_readdir` over sockets, pipes, or `/dev/*` non-directory fds — those
  return `ENOTDIR`.
- No `seekdir`/`telldir` beyond the standard cookie mechanism.
- No symlink dirent type (we have no symlinks; never emit `SYMBOLIC_LINK`).

## Architecture

### On-wire layout (WASI Preview 1)

`fd_readdir(fd, buf, buf_len, cookie) -> (errno, bufused)`. In the wasmi
`func_wrap` signature this is:

```
fd_readdir(fd: i32, buf_ptr: i32, buf_len: i32, cookie: i64, bufused_ptr: i32) -> i32
```

The buffer is filled with a packed sequence of entries. Each entry is a 24-byte
`__wasi_dirent_t` header immediately followed by `d_namlen` bytes of name (no
NUL, no padding between header and name; the next header starts right after the
name):

```
struct __wasi_dirent_t {   // sizeof = 24
    u64 d_next;    // offset 0  — cookie to pass to resume AFTER this entry
    u64 d_ino;     // offset 8  — inode (0 is acceptable)
    u32 d_namlen;  // offset 16 — name length in bytes
    u8  d_type;    // offset 20 — __wasi_filetype_t
    u8  _pad[3];   // offset 21..24
}
```

`__wasi_filetype_t` values we emit:

| VfsKind | filetype | value |
|---|---|---|
| `Dir` | `DIRECTORY` | 3 |
| `Reg` | `REGULAR_FILE` | 4 |
| `Device` | `CHARACTER_DEVICE` | 2 |
| (`.` and `..`) | `DIRECTORY` | 3 |

### Cookie / resumption semantics

- The start cookie is `0` (`__WASI_DIRCOOKIE_START`).
- Build the full entry list once per call: synthetic `.` (index 0) then `..`
  (index 1), then the VFS entries (indices 2..N).
- The entry at index `i` is written with `d_next = i + 1`. A subsequent call
  with `cookie = K` therefore **skips the first K entries** and resumes at index
  `K`.
- Fill the buffer until the next entry's `header + name` would not fit. **A
  partial final entry is allowed**: write as many bytes as fit (header possibly
  truncated, name possibly truncated) and stop. wasi-libc re-reads with a larger
  buffer.
- `bufused` = total bytes written. The caller knows enumeration is complete when
  `bufused < buf_len`. When `bufused == buf_len`, it calls again with the cookie
  of the last fully/partially written entry. (Standard behavior; just emit
  `d_next` correctly and the libc handles the loop.)

### Async bridge

Enumeration is async: `crate::vfs::readdir(&path).await` and
`crate::vfs::stat(&entry_path).await`. Host fns cannot `.await`, so `fd_readdir`
**traps with a new `SuspendReason`**, handled in `Fiber::run`, exactly mirroring
the existing `SuspendReason::ReadDir` handler (`kernel/src/wasm/fiber.rs:373`).
The only difference vs that handler is the on-wire layout (WASI dirent + cookie
instead of the 12-byte `ruos` record) and the `cookie` skip.

Opening the directory in `path_open(O_DIRECTORY)` also needs an async `stat`
(to confirm it is a directory), so that path traps too — either via a new
`SuspendReason::OpenDir` or by reusing the existing `PathOpen` flow with a flag.
`OpenDir` is cleaner; see Component 2.

## Components

### 1. `kernel/src/wasm/state.rs` — `FdEntry::Dir`

Add a directory variant carrying the resolved absolute path (string is the
simplest stable handle; we re-enumerate per call, which is fine for our scale):

```rust
pub enum FdEntry {
    StdoutConsole,
    Vfs(crate::vfs::Fd),
    Socket(usize),
    Dir(alloc::string::String),   // resolved absolute path
}
```

Update any exhaustive `match` on `FdEntry` (grep for `FdEntry::`) — at minimum
`fd_close`, `fd_fdstat_get`, `fd_filestat_get`, `fd_write`/`fd_read` (Dir →
`EISDIR`/`ENOTDIR` as appropriate; `fd_read`/`fd_write` on a dir → return `21`
`EISDIR`).

### 2. `kernel/src/wasm/host/path.rs` — `path_open(O_DIRECTORY)`

When `oflags & OFLAGS_DIRECTORY != 0`, do not go through the file `PathOpen`
flow. Trap with a new suspend reason that stats the path:

```rust
Err(Error::host(SuspendReason::OpenDir {
    path,                       // already resolve_cwd'd
    opened_fd_ptr: opened_fd_ptr as u32,
}))
```

Handler (Component 5): `vfs::stat(&path).await`; if `kind == Dir`, allocate a
free fd slot with `FdEntry::Dir(path)` and write the fd index to
`opened_fd_ptr`, return `0`. If stat is Ok but not a dir → `54` (`ENOTDIR`). If
stat errors → `44` (`ENOENT`).

Keep the existing non-`O_DIRECTORY` behavior untouched.

### 3. `kernel/src/wasm/host/fd.rs` — `fd_readdir` host fn + fdstat/close

- New `fd_readdir(caller, fd, buf_ptr, buf_len, cookie, bufused_ptr) -> i32`:
  - Resolve `fd` in the fd table. If it is not `FdEntry::Dir(path)`, return `54`
    (`ENOTDIR`). (wasi-libc only calls this on dir fds, but be defensive.)
  - Trap with `SuspendReason::FdReadDir { path: path.clone(), cookie: cookie as u64, buf_ptr: buf_ptr as u32, buf_len: buf_len as usize, bufused_ptr: bufused_ptr as u32 }`.
- `fd_fdstat_get`: for `FdEntry::Dir`, report `fs_filetype = 3` (`DIRECTORY`).
  This is **load-bearing**: wasi-libc's `fdopendir` verifies the fd is a
  directory via `fd_fdstat_get` before issuing `fd_readdir`.
- `fd_close`: for `FdEntry::Dir`, just clear the slot (no VFS handle to release).
- Register in this file's `link()`:
  `linker.func_wrap("wasi_snapshot_preview1", "fd_readdir", fd_readdir)?;`

### 4. `kernel/src/wasm/suspend.rs` — new reasons

```rust
OpenDir   { path: String, opened_fd_ptr: u32 },
FdReadDir { path: String, cookie: u64, buf_ptr: u32, buf_len: usize, bufused_ptr: u32 },
```

### 5. `kernel/src/wasm/fiber.rs` — handlers

- `OpenDir`: as in Component 2. Allocate fd, write index, return errno.
- `FdReadDir`: copy the structure of the existing `ReadDir` arm
  (`fiber.rs:373`). Differences:
  - Prepend synthetic `.` and `..` entries (both `DIRECTORY`).
  - For each entry at running index `i` (starting at 0), emit the 24-byte
    `__wasi_dirent_t` with `d_next = i + 1`, `d_ino = 0`, `d_namlen`,
    `d_type` from `VfsKind`, then the name bytes.
  - Skip the first `cookie` entries (do not emit them).
  - Stop when the buffer is full; allow a truncated final entry; track
    `bufused`.
  - `vfs::readdir` error → write `bufused = 0`, return `44` (`ENOENT`); empty dir
    is still success with `.`/`..` (or just `bufused = 0` after cookie ≥ count).

### 6. Testing (`user/` + `Makefile` / `tests/`)

Add a smoke that exercises the **std** path (not `ruos.readdir`), so the test
actually proves WASI works. Two options — pick the lighter:

- Extend `user/init` (or `user-bin/smoke.sh` flow) to run a tiny program that
  calls `std::fs::read_dir("/bin")`, counts entries, and prints a marker line
  like `readdir-std: <N> entries`. Then have `make run-test` grep for it
  (`TEST_PASS_READDIR`).
- Or add a minimal `user/readdir-smoke` crate doing the same and wire it into
  the smoke init script.

The point: the assertion must go through `fd_readdir`, e.g. by using
`std::fs::read_dir` directly (no `ruos.readdir` import in the test crate).

## Error handling

WASI errno values used (decimal): `0` success, `8` `EBADF`, `21` `EISDIR`,
`28` `EINVAL`, `44` `ENOENT`, `54` `ENOTDIR`. Match the conventions already used
across `kernel/src/wasm/host/*` (e.g. the existing `ReadDir` arm returns `44`).

## Done criteria

- A `wasm32-wasip1` `std` binary calling `std::fs::read_dir("/bin")` lists the
  directory correctly (entries match `ls /bin`), with `.`/`..` filtered by std.
- `walkdir` over a small tree (recompiled to `wasm32-wasip1`) recurses without
  error.
- `make run-test` passes and emits the new readdir marker.
- All existing tests (`make run-test`, `run-ssh-test`, `run-passwd-test`) still
  pass; `ls`/`find`/`du` (still on `ruos.readdir`) unchanged.

# Implementation Plan

> Per `CLAUDE.md`: create a feature branch first (do not work on `master`), and
> add one `CHANGELOG/NN-26-05-31-<slug>.md` entry **per task** using the next
> free `NN` (current highest is `169`, so start at `170`). Do not commit/push
> unless explicitly asked.

## Task 1 — `FdEntry::Dir` + match arms
Add the variant in `state.rs`; fix every `match` on `FdEntry` (`fd_close`,
`fd_fdstat_get`, `fd_filestat_get`, `fd_read`, `fd_write`). `fd_read`/`fd_write`
on a Dir → `21` (`EISDIR`). Build must be clean (`make iso`). Changelog `170`.

## Task 2 — `OpenDir` suspend + `path_open(O_DIRECTORY)`
Add `SuspendReason::OpenDir`; trap from `path_open` on `OFLAGS_DIRECTORY`; handle
in `fiber.rs` (stat → alloc `FdEntry::Dir` or `ENOTDIR`/`ENOENT`). `fd_fdstat_get`
reports `DIRECTORY` for Dir fds. Changelog `171`.

## Task 3 — `fd_readdir` host fn + `FdReadDir` handler
Add `SuspendReason::FdReadDir`, the host fn in `fd.rs`, registration in `link()`,
and the `fiber.rs` arm with the WASI dirent layout + cookie skip + `.`/`..`.
Changelog `172`.

## Task 4 — Smoke test + run-test marker
Add the `std::fs::read_dir` smoke (Component 6), wire the grep into `make
run-test`, rebuild the affected `.wasm` into `user-bin/`. Changelog `173`.

## Task 5 — Docs
Update `README.md` status/notes if appropriate and
`docs/superpowers/roadmap-rust-os.md` to note WASI `fd_readdir` landed.
Changelog `174`.

## Notes for the implementer

- **Template to copy:** the existing `SuspendReason::ReadDir` arm at
  `kernel/src/wasm/fiber.rs:373` already does async `vfs::readdir` + per-entry
  `vfs::stat` and writes to wasm memory via `self.write_to_memory` /
  `self.write_u32`. `FdReadDir` is the same minus stat-for-size, plus the dirent
  layout and the cookie skip.
- **Helpers:** `wasm_memory(&caller)` lives in `wasm/host/lifecycle.rs`;
  `resolve_cwd(&caller.data().cwd, s)` in `wasm/host/proc.rs`; `OFLAGS_DIRECTORY`
  and the rights/oflag constants in `wasm/host/path.rs`.
- **VFS API:** `crate::vfs::readdir(&path).await -> Result<Vec<DirEntry>, _>`
  where each entry has `.name: String` and `.kind: VfsKind` (`Reg | Dir |
  Device`). `crate::vfs::stat(&path).await -> Stat { size, .. }` (not needed for
  dirent unless you choose to fill `d_ino`).
- **wasi-libc gotcha:** `fdopendir` checks `fd_fdstat_get(...).fs_filetype ==
  DIRECTORY` before `fd_readdir`. If Task 2 forgets that, `read_dir` fails with
  `ENOTDIR` *before* `fd_readdir` is ever called — test that path explicitly.
- **Preopen:** `fd_prestat`/`fd_prestat_dir_name` already expose `/` at fd 3
  (see `fd.rs`), so relative `read_dir("subdir")` resolves correctly via
  `resolve_cwd`. `path_open` ignores `_dir_fd` and resolves against cwd/absolute
  — keep that behavior.
- **Don't break `ruos.readdir`:** it has a different (12-byte) record layout and
  is still used by `ls`/`find`/`du`/`grep`. Leave it entirely alone.
