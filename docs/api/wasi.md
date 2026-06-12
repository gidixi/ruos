# Module `wasi_snapshot_preview1` — WASI Preview 1

The standard WASI Preview 1 surface, enough for Rust `std` on `wasm32-wasip1`.
Runtime: **wasmi**. Sources: `kernel/src/wasm/host/{lifecycle,clock,random,fd,path}.rs`.

You normally don't call these directly — Rust `std` (`std::fs`, `std::env`,
`std::time`, `println!`) does. Listed for completeness.

**Last reviewed:** 2026-06-12.

---

## Lifecycle  (`lifecycle.rs`)

| Function | Meaning |
|----------|---------|
| `args_sizes_get(argc_ptr, argv_buf_size_ptr) -> i32` | argv count + buffer size. |
| `args_get(argv_ptr, argv_buf_ptr) -> i32` | argv pointer array + NUL-terminated strings. |
| `environ_sizes_get(environc_ptr, environ_buf_size_ptr) -> i32` | env count + buffer size. |
| `environ_get(environ_ptr, environ_buf_ptr) -> i32` | `KEY=VALUE\0` strings. Includes `PWD=<cwd>` (set by the kernel at exec). |
| `proc_exit(code) -> !` | Terminate the tool with `code`. |
| `poll_oneoff(in_ptr, out_ptr, nsubs, nevents_ptr) -> i32` | Clock subscriptions (sleep) only; suspends. `28` EINVAL. |
| `sched_yield() -> i32` | Cooperative yield (no-op on single-core). |

> `environ_get`/`PWD` + the `ruos_rt::init()` shim are how a tool's relative paths
> resolve against the shell's cwd. See [path resolution](#path-resolution).
>
> **Window/CLI apps on Wasmtime** (`kernel/src/wasm/wt/wasi.rs` shim):
> `environ_*` reads `WtState.env` (`KEY=VALUE` entries). Classic `.cwasm` tools
> get an **empty** environ (no `PWD` injection on this runtime); threaded
> modules (see below) get `RAYON_NUM_THREADS=<ComputeApp cores>` injected by
> the kernel at exec.

## wasi-threads  (Wasmtime only, `kernel/src/wasm/wt/threads.rs`)

Modules built for `wasm32-wasip1-threads` import their linear memory
(`env::memory`, shared) and the spawn host fn below; the kernel detects the
shared-memory import in `run_cwasm` and runs the module's `_start` on a
cooperative fiber (MT Fase 2).

| Function | Meaning |
|----------|---------|
| `("wasi", "thread-spawn")(start_arg: i32) -> i32` | Spawn a new thread: the host re-instantiates the module against the same shared memory and calls its exported `wasi_thread_start(tid, start_arg)`. Returns the new `tid > 0`, or a negative errno. **Current status: stub — always returns `-1`** (spawn lands with MT Fase 2 Task 5; `pthread_create` fails cleanly until then). |

## Clock  (`clock.rs`)

| Function | Meaning |
|----------|---------|
| `clock_time_get(clock_id, precision, time_ptr) -> i32` | `u64` ns since boot (10 ms resolution @ 100 Hz). |
| `clock_res_get(clock_id, res_ptr) -> i32` | Resolution (`10_000_000` ns). |

> **Window apps (Wasmtime, `kernel/src/wasm/wt/wasi.rs` shim):** same surface,
> but `clock_id 0` (REALTIME) returns **unix-epoch ns** (anchored to the RTC at
> first use) so `SystemTime::now()` is wall-clock — required by in-app TLS
> (rustls) for certificate-validity checks. Other ids stay ns-since-boot.
>
> **Window-app stdout/stderr** (`println!` → `fd_write`): serial + framebuffer
> console **and** the kernel log ring (`dmesg`) **and** netconsole (when the
> feature is on) — so app prints stay visible on real hardware where the
> desktop covers the framebuffer and there is no serial port.

## Random  (`random.rs`)

| Function | Meaning |
|----------|---------|
| `random_get(buf_ptr, buf_len) -> i32` | ChaCha20 CSPRNG (RDRAND-seeded) bytes. `28` EINVAL. |

## File descriptors  (`fd.rs`)

| Function | Meaning |
|----------|---------|
| `fd_write(fd, iovs_ptr, iovs_len, nwritten_ptr) -> i32` | Write iovecs. Console writes sync; VFS/socket suspend. `8` EBADF, `21` EISDIR. |
| `fd_read(fd, iovs_ptr, iovs_len, nread_ptr) -> i32` | Read iovecs. VFS/socket suspend. |
| `fd_seek(fd, offset, whence, newoffset_ptr) -> i32` | Seek (0=SET,1=CUR,2=END). |
| `fd_close(fd) -> i32` | Close VFS/socket fd. |
| `fd_readdir(fd, buf_ptr, buf_len, cookie, bufused_ptr) -> i32` | Directory entries (suspends). `54` ENOTDIR. |
| `fd_filestat_get(fd, buf_ptr) -> i32` | 64-byte `filestat` (filetype@16, size@32). |
| `fd_fdstat_get(fd, stat_ptr) -> i32` | 24-byte `fdstat`. |
| `fd_prestat_get(fd, stat_ptr) -> i32` | Preopen info — only fd 3 (`/`). |
| `fd_prestat_dir_name(fd, path_ptr, path_len) -> i32` | Preopen name — fd 3 → `/`. |

## Paths  (`path.rs`)

| Function | Meaning |
|----------|---------|
| `path_open(dir_fd, dir_flags, path_ptr, path_len, oflags, rights_base, rights_inheriting, fd_flags, opened_fd_ptr) -> i32` | Open file/dir (suspends). `44` ENOENT, `76` ENOTCAPABLE. |
| `path_unlink_file(dir_fd, path_ptr, path_len) -> i32` | Delete file. |
| `path_create_directory(dir_fd, path_ptr, path_len) -> i32` | mkdir. |
| `path_remove_directory(dir_fd, path_ptr, path_len) -> i32` | rmdir. |
| `path_filestat_get(dir_fd, flags, path_ptr, path_len, buf_ptr) -> i32` | stat. |
| `path_rename(old_fd, old_path_ptr, old_path_len, new_fd, new_path_ptr, new_path_len) -> i32` | rename. |

## Sockets

| Function | Meaning |
|----------|---------|
| `sock_accept(fd, flags, new_fd_ptr) -> i32` | Accept on a listening socket (suspends). |

---

## Path resolution

There is ONE preopen: fd 3 = `/`. wasi-libc resolves the program's path against it
and passes the kernel the remainder, so the kernel resolves WASI paths against `/`
(stateless — `resolve_at` in `path.rs`). Relative-to-cwd paths still work because
the kernel injects `PWD=<cwd>` and each tool's `ruos_rt::init()` calls
`set_current_dir(PWD)` at startup, so wasi-libc roots relative paths at the real cwd
before stripping. (CLI command resolution uses `ruos.readdir`/`exec`, which resolve
against the kernel cwd directly.)
