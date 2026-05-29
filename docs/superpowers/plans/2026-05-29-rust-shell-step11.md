# Shell Step 11 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Real userland shell (`shell.wasm`) reads `/etc/init.sh` (Limine module) and exec's commands via custom `ruos_exec` host fn that spawns child Fibers cooperatively. 4 user crates (shell + ls + cat + echo) shipped as Limine modules.

**Architecture:** `shell.wasm` (compiled `wasm32-wasip1`) runs as embassy task. Builtin commands (`cd`, `pwd`, `exit`, `help`) are shell-internal Rust. External commands resolve to `/bin/<name>.wasm` via PATH lookup, then `ruos_exec` host fn traps `SuspendReason::Exec`; Fiber dispatch reads the wasm bytes from VFS, instantiates a child `Fiber`, runs it to completion, returns exit code. Same pattern as Step 10.5 SuspendReason but for child spawning.

**Tech Stack:** wasmi 1.0.9 fiber rewrite (Step 10.5), VFS `readdir`+`stat` (Step 10.5b), `wasm32-wasip1` Rust user crates, Limine modules. No new kernel deps.

**Spec:** `docs/superpowers/specs/2026-05-29-rust-shell-step11-design.md`

**Branch:** `feature/step-11-shell` (already created)

**Build host:** WSL Ubuntu, all commands via:
```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && <cmd>'
```

**Test model:** kernel-TDD via `make run-test` HELLO sentinel.

**Changelog rule:** Spec = 79. Plan = 80. Implementer tasks: 81 (T1), 82 (T2), 83 (T3).

**Git identity (mandatory):**
```
git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit ...
```
Co-author trailer at end of every commit:
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

**wasmi 1.0.9 + Fiber patterns established by Step 10.5:**
- `wasmi::Error::i32_exit(N)` / `wasmi::Error::host(SuspendReason)`
- `wasmi::Error` (no `errors::` submodule)
- Memory: `mem.read(&caller, ...)` / `mem.write(&caller, ...)` (Caller = AsContext[Mut])
- `wasmi::ResumableCall::{Finished, HostTrap(state)}`
- `state.host_error() -> &Error`, downcast to `SuspendReason`
- Fiber's `dispatch` matches `SuspendReason` variants + writes results to wasm memory via `self.instance.get_export(memory)`

**VFS APIs available (Step 10.5b):**
- `vfs::read_all(path) -> Vec<u8>` via Fiber's existing `read_all` helper in wasm/mod.rs
- `vfs::readdir(path) -> Vec<VfsDirent>` (new, Step 10.5b commit 77)
- `vfs::stat(path) -> VfsStat` (new, Step 10.5b commit 78)
- `VfsKind { Dir, Reg, Device }`

**HELLO progression per task:**
| Task | HELLO before | HELLO after |
|------|-------------|-------------|
| 1 | `ruos: real ping-pong (no preload)` | `init.wasm: argv0=/init.wasm` |
| 2 | `init.wasm: argv0=/init.wasm` | `shell: init.sh complete` |
| 3 | `shell: init.sh complete` | `shell: init.sh complete` (unchanged) |

---

## File Structure

**New kernel files:**
- `kernel/src/wasm/host/proc.rs` — `ruos_exec` + `ruos_readdir` host fns

**Files modified (kernel):**
- `kernel/src/wasm/suspend.rs` — add `SuspendReason::Exec` + `ReadDir` variants
- `kernel/src/wasm/fiber.rs` — add dispatch arms for Exec/ReadDir; add `Fiber::set_args` method
- `kernel/src/wasm/state.rs` — `RuntimeState.args` populated by `set_args` (currently empty Vec)
- `kernel/src/wasm/host/lifecycle.rs` — `args_sizes_get` + `args_get` populate from `state.args` (currently return 0/0)
- `kernel/src/wasm/host/mod.rs` — register `proc` module
- `kernel/src/executor/mod.rs` — spawn `wasm_task("/bin/shell.wasm")`; drop `kbd_echo_task` (T3)
- `kernel/src/main.rs` — verify net + init order unchanged
- `kernel/Cargo.toml` — no changes expected

**New `user/` files:**
- `user/shell/{Cargo.toml,src/main.rs}` — shell.wasm
- `user/ls/{Cargo.toml,src/main.rs}` — ls.wasm
- `user/cat/{Cargo.toml,src/main.rs}` — cat.wasm
- `user/echo/{Cargo.toml,src/main.rs}` — echo.wasm

**Build outputs (committed binaries):**
- `user-bin/shell.wasm`
- `user-bin/ls.wasm`
- `user-bin/cat.wasm`
- `user-bin/echo.wasm`
- `user-bin/init.sh` — boot script (plain text file)

**Modified:**
- `user/Cargo.toml` — workspace members
- `limine.conf` — declare 5 new modules
- `Makefile` — build 4 wasms + include init.sh + 5 ISO copies + HELLO sentinel

---

## Task 1: Host fns `ruos_exec` + `ruos_readdir` + lifecycle args

**Files:**
- Create: `kernel/src/wasm/host/proc.rs`
- Modify: `kernel/src/wasm/host/mod.rs` (link proc)
- Modify: `kernel/src/wasm/suspend.rs` (Exec + ReadDir variants)
- Modify: `kernel/src/wasm/fiber.rs` (dispatch arms + `set_args`)
- Modify: `kernel/src/wasm/state.rs` (no behavior change; verify args field)
- Modify: `kernel/src/wasm/host/lifecycle.rs` (args_* return real data)
- Modify: `user/init/src/main.rs` (print argv[0] to verify)
- Modify: `Makefile` (HELLO sentinel)

**Smoke contract:** `init.wasm: argv0=/init.wasm` (proves args_get returns real argv).

This task validates the host fn surface architecturally before any external `.wasm` exists. `ruos_exec` will work but no caller exercises it yet (T2 uses it). `args_*` real impl exercised by extending init.wasm.

- [ ] **Step 1.1: HELLO bump**

Edit `Makefile`:
```makefile
HELLO := init.wasm: argv0=/init.wasm
```
(was `ruos: real ping-pong (no preload)`)

- [ ] **Step 1.2: Verify failing**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 30 make run-test 2>&1 | tail -10'
```

Expected: error.

- [ ] **Step 1.3: Extend `SuspendReason` enum**

Edit `kernel/src/wasm/suspend.rs`. Add two variants alongside existing ones:

```rust
    Exec {
        path: String,
        argv: Vec<Vec<u8>>,
        exit_code_ptr: u32,
    },
    ReadDir {
        path: String,
        buf_ptr: u32,
        buf_len: usize,
        nread_ptr: u32,
    },
```

(Place before the closing brace. The enum already has `Sleep`/`Sock*`/`Vfs*`/`Path*`/`Kbd*` from Step 10.5.)

- [ ] **Step 1.4: Create `kernel/src/wasm/host/proc.rs`**

```rust
//! Custom (non-WASIX) host fns for process control:
//!   ruos_exec   — spawn a child Fiber on a `.wasm` path, wait for exit
//!   ruos_readdir — list a VFS directory into a flat dirent buffer
//!
//! Both are linked under the module name "ruos" (not
//! "wasi_snapshot_preview1") so guest libs can opt in via extern
//! "C" #[link(wasm_import_module = "ruos")].

use wasmi::{Caller, Linker, Error};
use alloc::string::String;
use alloc::vec::Vec;
use crate::wasm::state::RuntimeState;
use crate::wasm::host::lifecycle::wasm_memory;
use crate::wasm::suspend::SuspendReason;

/// ruos_exec(path_ptr, path_len, argv_ptr, argv_len, exit_code_ptr) -> errno
///
/// argv format in wasm memory:
///   u32 count
///   <count copies of (u32 offset, u32 length)>
///   <strings packed at the declared offsets, relative to argv_ptr>
pub fn ruos_exec(
    caller: Caller<'_, RuntimeState>,
    path_ptr: i32,
    path_len: i32,
    argv_ptr: i32,
    argv_len: i32,
    exit_code_ptr: i32,
) -> Result<i32, Error> {
    let mem = wasm_memory(&caller)?;
    // Read path.
    let mut path_buf = alloc::vec![0u8; path_len as usize];
    mem.read(&caller, path_ptr as usize, &mut path_buf)
        .map_err(|_| Error::i32_exit(-1))?;
    let path = core::str::from_utf8(&path_buf)
        .map_err(|_| Error::i32_exit(-1))?
        .to_string();
    // Read argv blob.
    let mut argv_blob = alloc::vec![0u8; argv_len as usize];
    mem.read(&caller, argv_ptr as usize, &mut argv_blob)
        .map_err(|_| Error::i32_exit(-1))?;
    // Decode argv: first 4 bytes = count.
    let argv = decode_argv(&argv_blob).unwrap_or_default();
    Err(Error::host(SuspendReason::Exec {
        path,
        argv,
        exit_code_ptr: exit_code_ptr as u32,
    }))
}

fn decode_argv(blob: &[u8]) -> Option<Vec<Vec<u8>>> {
    if blob.len() < 4 { return None; }
    let count = u32::from_le_bytes([blob[0], blob[1], blob[2], blob[3]]) as usize;
    let mut out = Vec::with_capacity(count);
    let table_start = 4usize;
    let table_end = table_start.checked_add(count.checked_mul(8)?)?;
    if blob.len() < table_end { return None; }
    for i in 0..count {
        let off = table_start + i * 8;
        let offset = u32::from_le_bytes([blob[off], blob[off+1], blob[off+2], blob[off+3]]) as usize;
        let length = u32::from_le_bytes([blob[off+4], blob[off+5], blob[off+6], blob[off+7]]) as usize;
        let end = offset.checked_add(length)?;
        if blob.len() < end { return None; }
        out.push(blob[offset..end].to_vec());
    }
    Some(out)
}

/// ruos_readdir(path_ptr, path_len, buf_ptr, buf_len, nread_ptr) -> errno
///
/// Output dirent format (per entry):
///   u8  kind  (0=Reg, 1=Dir, 2=Device)
///   u8  reserved
///   u16 name_len
///   u64 size
///   u8[name_len] name
pub fn ruos_readdir(
    caller: Caller<'_, RuntimeState>,
    path_ptr: i32,
    path_len: i32,
    buf_ptr: i32,
    buf_len: i32,
    nread_ptr: i32,
) -> Result<i32, Error> {
    let mem = wasm_memory(&caller)?;
    let mut path_buf = alloc::vec![0u8; path_len as usize];
    mem.read(&caller, path_ptr as usize, &mut path_buf)
        .map_err(|_| Error::i32_exit(-1))?;
    let path = core::str::from_utf8(&path_buf)
        .map_err(|_| Error::i32_exit(-1))?
        .to_string();
    Err(Error::host(SuspendReason::ReadDir {
        path,
        buf_ptr: buf_ptr as u32,
        buf_len: buf_len as usize,
        nread_ptr: nread_ptr as u32,
    }))
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker
        .func_wrap("ruos", "exec", ruos_exec)?
        .func_wrap("ruos", "readdir", ruos_readdir)?;
    Ok(())
}
```

- [ ] **Step 1.5: Register `proc` in `host/mod.rs`**

Edit `kernel/src/wasm/host/mod.rs`. Add `pub mod proc;` at the top and call `proc::link(linker)?;` in `install`:

```rust
pub mod lifecycle;
pub mod fd;
pub mod path;
pub mod clock;
pub mod random;
pub mod sock;
pub mod proc;

pub fn install(linker: &mut wasmi::Linker<RuntimeState>) -> Result<(), wasmi::Error> {
    lifecycle::link(linker)?;
    fd::link(linker)?;
    path::link(linker)?;
    clock::link(linker)?;
    random::link(linker)?;
    sock::link(linker)?;
    proc::link(linker)?;
    Ok(())
}
```

(Adjust module list to match what's actually in mod.rs today — read the file first.)

- [ ] **Step 1.6: Extend `Fiber` with `set_args` + Exec/ReadDir dispatch**

Edit `kernel/src/wasm/fiber.rs`.

Add a method on `Fiber`:

```rust
impl Fiber {
    pub fn set_args(&mut self, args: alloc::vec::Vec<alloc::vec::Vec<u8>>) {
        self.store.data_mut().args = args;
    }
}
```

Place after the `new()` constructor.

Extend the `dispatch` match:

```rust
            SuspendReason::Exec { path, argv, exit_code_ptr } => {
                let bytes = match crate::wasm::read_all(&path).await {
                    Ok(b) => b,
                    Err(_) => {
                        let _ = self.write_u32(exit_code_ptr, u32::MAX);
                        return 8; // ENOENT-ish (use 44 instead if your map has it)
                    }
                };
                let mut child = match crate::wasm::fiber::Fiber::new(&bytes) {
                    Ok(c) => c,
                    Err(_) => {
                        let _ = self.write_u32(exit_code_ptr, u32::MAX);
                        return 71; // ENOEXEC
                    }
                };
                child.set_args(argv);
                let code = child.run().await as i32;
                let _ = self.write_u32(exit_code_ptr, code as u32);
                0
            }
            SuspendReason::ReadDir { path, buf_ptr, buf_len, nread_ptr } => {
                let entries = match crate::vfs::readdir(&path).await {
                    Ok(v) => v,
                    Err(_) => {
                        let _ = self.write_u32(nread_ptr, 0);
                        return 44; // ENOENT
                    }
                };
                let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
                for e in entries.iter() {
                    let name_bytes = e.name.as_bytes();
                    if name_bytes.len() > u16::MAX as usize { continue; }
                    let kind_byte: u8 = match e.kind {
                        crate::vfs::VfsKind::Reg => 0,
                        crate::vfs::VfsKind::Dir => 1,
                        crate::vfs::VfsKind::Device => 2,
                    };
                    // stat for size
                    let entry_path = {
                        let mut s = path.clone();
                        if !s.ends_with('/') { s.push('/'); }
                        s.push_str(&e.name);
                        s
                    };
                    let size: u64 = match crate::vfs::stat(&entry_path).await {
                        Ok(s) => s.size,
                        Err(_) => 0,
                    };
                    out.push(kind_byte);
                    out.push(0);
                    out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
                    out.extend_from_slice(&size.to_le_bytes());
                    out.extend_from_slice(name_bytes);
                }
                let n = out.len().min(buf_len);
                let _ = self.write_to_memory(buf_ptr, &out[..n]);
                let _ = self.write_u32(nread_ptr, n as u32);
                0
            }
```

`crate::wasm::read_all` may currently be private in `wasm/mod.rs`. If so, make it `pub(crate) async fn read_all(...)` or add a thin re-export. Same applies if it has a different name.

- [ ] **Step 1.7: Real `args_sizes_get` / `args_get` in `lifecycle.rs`**

In `kernel/src/wasm/host/lifecycle.rs`, replace the stub `args_sizes_get` and `args_get` (currently return 0/0 / noop) with:

```rust
pub fn args_sizes_get(
    caller: Caller<'_, RuntimeState>,
    argc_ptr: i32,
    argv_buf_size_ptr: i32,
) -> Result<i32, Error> {
    let state = caller.data();
    let argc: u32 = state.args.len() as u32;
    // WASI convention: argv_buf is the concatenation of all argv
    // strings, each terminated with a '\0'. Compute total bytes.
    let argv_buf: u32 = state.args.iter().map(|a| a.len() as u32 + 1).sum();
    let mem = wasm_memory(&caller)?;
    write_u32(&mem, &caller, argc_ptr, argc)?;
    write_u32(&mem, &caller, argv_buf_size_ptr, argv_buf)?;
    Ok(0)
}

pub fn args_get(
    caller: Caller<'_, RuntimeState>,
    argv_ptr: i32,
    argv_buf_ptr: i32,
) -> Result<i32, Error> {
    let state = caller.data();
    let args = state.args.clone();
    let mem = wasm_memory(&caller)?;
    let mut cursor = argv_buf_ptr as u32;
    for (i, arg) in args.iter().enumerate() {
        // argv[i] = pointer to argv_buf+cursor
        write_u32(&mem, &caller, argv_ptr + (i as i32) * 4, cursor)?;
        // copy arg + NUL
        let mut owned = arg.clone();
        owned.push(0u8);
        mem.write(&caller, cursor as usize, &owned)
            .map_err(|_| Error::i32_exit(-1))?;
        cursor += owned.len() as u32;
    }
    Ok(0)
}
```

(The original stubs returned `Ok(0)` without writing memory. The new versions write argc/argv_buf_size and the pointer table + bytes.)

- [ ] **Step 1.8: Verify `RuntimeState.args` exists and is `Vec<Vec<u8>>`**

Read `kernel/src/wasm/state.rs`. If `args` is already `pub args: Vec<Vec<u8>>` (it should be from Step 10 Task 2), no change. If it doesn't exist, add it:

```rust
pub args: alloc::vec::Vec<alloc::vec::Vec<u8>>,
```

In `RuntimeState::new()`, initialize as `Vec::new()`.

- [ ] **Step 1.9: Set argv[0] when spawning init.wasm**

Edit the wasm_task spawn in `kernel/src/executor/mod.rs`. Currently spawns `wasm_task("/init.wasm")`. We need the `Fiber` instance to have its argv set to `["/init.wasm"]` so init.wasm's `args[0]` is its path.

Find `kernel/src/wasm/mod.rs::run_at`. The current impl creates a Fiber and runs it. After `Fiber::new(&bytes)`, before `fb.run().await`, add:

```rust
fb.set_args(alloc::vec![path.as_bytes().to_vec()]);
```

That makes argv[0] = the path string (without null terminator; lifecycle::args_get appends NUL).

- [ ] **Step 1.10: Update `user/init/src/main.rs` to print argv[0]**

Add at the top of `main()`:

```rust
    let args: Vec<String> = std::env::args().collect();
    if let Some(arg0) = args.get(0) {
        println!("init.wasm: argv0={}", arg0);
    }
```

Place before the welcome banner.

- [ ] **Step 1.11: Rebuild + test**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make user-wasm && make build 2>&1 | tail -15 && timeout 30 make run-test 2>&1 | tail -25'
```

Expected serial tail (cropped):
```
init.wasm: argv0=/init.wasm
\x1b[1;32m╔══════════════════════════════════╗
║         Welcome to ruos          ║
║   wasm32-wasip1 / WASIX host     ║
╚══════════════════════════════════╝\x1b[0m
init.wasm: slept ok
... (rest unchanged)
```

Sentinel `init.wasm: argv0=/init.wasm` PASS.

If you see `init.wasm: argv0=` (empty) or no argv0 line at all, args_get isn't being called or returns empty. Trace by adding `kprintln!` in `lifecycle::args_get`.

- [ ] **Step 1.12: Changelog + commit**

Create `CHANGELOG/81-26-05-29-shell-host-fns.md`:

```markdown
# 81 — ruos_exec + ruos_readdir host fns + lifecycle args real (Step 11 Task 1)

**Data:** 2026-05-29

## Cosa

- `kernel/src/wasm/host/proc.rs` (nuovo): `ruos_exec` + `ruos_readdir`
  host fns trapsano `SuspendReason::Exec` e `SuspendReason::ReadDir`.
  Linked under module "ruos" (non WASIX standard).
- `kernel/src/wasm/suspend.rs`: aggiunte varianti `Exec` e `ReadDir`.
- `kernel/src/wasm/fiber.rs`: nuovo metodo `Fiber::set_args`; dispatch
  arms per Exec (spawn child Fiber via vfs::read_all + run().await) e
  ReadDir (vfs::readdir + stat per entry, encoding dirent format).
- `kernel/src/wasm/host/lifecycle.rs`: `args_sizes_get`/`args_get`
  ora popolano da `state.args` invece di ritornare zero.
- `kernel/src/wasm/mod.rs::run_at`: chiama `fb.set_args(["/path"])` prima
  di run, così argv[0] è il path del modulo.
- `user/init/src/main.rs`: prints argv[0] subito dopo entrare in main.
- HELLO → `init.wasm: argv0=/init.wasm`.

## Perché

Primo task dello Step 11. Mette in piedi le host fns custom che T2
userà (shell.wasm + ls.wasm chiameranno ruos_exec / ruos_readdir).
T1 valida architetturalmente exec + readdir via init.wasm (caller
esistente, no new wasm).

## File toccati

- kernel/src/wasm/host/proc.rs (nuovo)
- kernel/src/wasm/host/mod.rs
- kernel/src/wasm/suspend.rs
- kernel/src/wasm/fiber.rs
- kernel/src/wasm/host/lifecycle.rs
- kernel/src/wasm/mod.rs
- user/init/src/main.rs
- user-bin/init.wasm (rigenerato)
- Makefile (HELLO)
- CHANGELOG/81-26-05-29-shell-host-fns.md (nuovo)
```

Commit:
```bash
git add kernel/src/wasm/ user/init/src/main.rs user-bin/init.wasm Makefile CHANGELOG/81-26-05-29-shell-host-fns.md
git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit -m "feat(rust): ruos_exec + ruos_readdir host fns + real args_*

New wasm/host/proc.rs introduces two custom host fns linked under
module 'ruos': ruos_exec spawns a child Fiber for a wasm path and
waits its exit code; ruos_readdir formats a VFS directory listing
into a flat dirent buffer. Both trap with SuspendReason variants
(Exec, ReadDir) and let Fiber::dispatch.await drive the work.

lifecycle::args_sizes_get + args_get now read from RuntimeState.args
(previously stubs returning 0). Fiber gains set_args(argv) for the
spawner to populate before run().

run_at sets argv[0] = the launched .wasm path, so init.wasm's
std::env::args() returns its own path. Sentinel proves end-to-end:
'init.wasm: argv0=/init.wasm'.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: 4 user crates (shell, ls, cat, echo) + init.sh + Limine wiring

**Files:**
- Create: `user/shell/{Cargo.toml,src/main.rs}`
- Create: `user/ls/{Cargo.toml,src/main.rs}`
- Create: `user/cat/{Cargo.toml,src/main.rs}`
- Create: `user/echo/{Cargo.toml,src/main.rs}`
- Create: `user-bin/init.sh`
- Modify: `user/Cargo.toml` (workspace members)
- Modify: `limine.conf` (5 new modules)
- Modify: `Makefile` (build 4 wasms + copy init.sh; HELLO)
- Modify: `kernel/src/executor/mod.rs` (spawn shell wasm_task)

**Smoke contract:** `shell: init.sh complete`.

- [ ] **Step 2.1: HELLO bump**

```makefile
HELLO := shell: init.sh complete
```

- [ ] **Step 2.2: Verify failing**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 30 make run-test 2>&1 | tail -10'
```

- [ ] **Step 2.3: Update `user/Cargo.toml` workspace**

```toml
[workspace]
resolver = "2"
members = ["init", "server", "client", "shell", "ls", "cat", "echo"]

[profile.release]
opt-level = "s"
lto = true
panic = "abort"
strip = true
```

- [ ] **Step 2.4: Create `user/echo/`**

```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && mkdir -p user/echo/src'
```

`user/echo/Cargo.toml`:
```toml
[package]
name = "echo"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "echo"
path = "src/main.rs"
```

`user/echo/src/main.rs`:
```rust
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    println!("{}", args.join(" "));
}
```

- [ ] **Step 2.5: Create `user/cat/`**

```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && mkdir -p user/cat/src'
```

`user/cat/Cargo.toml`:
```toml
[package]
name = "cat"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "cat"
path = "src/main.rs"
```

`user/cat/src/main.rs`:
```rust
use std::io::Read;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = match args.get(1) {
        Some(p) => p.clone(),
        None => {
            eprintln!("cat: missing path");
            std::process::exit(1);
        }
    };
    match std::fs::File::open(&path) {
        Ok(mut f) => {
            let mut buf = Vec::new();
            if let Err(e) = f.read_to_end(&mut buf) {
                eprintln!("cat: {}: {}", path, e);
                std::process::exit(1);
            }
            // Print as UTF-8 if possible; otherwise hex dump first 32 bytes.
            if let Ok(s) = std::str::from_utf8(&buf) {
                print!("{}", s);
            } else {
                print!("(binary, {} bytes)", buf.len());
            }
        }
        Err(e) => {
            eprintln!("cat: {}: {}", path, e);
            std::process::exit(1);
        }
    }
}
```

- [ ] **Step 2.6: Create `user/ls/`**

```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && mkdir -p user/ls/src'
```

`user/ls/Cargo.toml`:
```toml
[package]
name = "ls"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "ls"
path = "src/main.rs"
```

`user/ls/src/main.rs`:
```rust
#[link(wasm_import_module = "ruos")]
extern "C" {
    fn readdir(
        path_ptr: u32, path_len: u32,
        buf_ptr: u32, buf_len: u32,
        nread_ptr: u32,
    ) -> i32;
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).cloned().unwrap_or_else(|| "/".to_string());
    let mut buf = vec![0u8; 8192];
    let mut nread: u32 = 0;
    let errno = unsafe {
        readdir(
            path.as_ptr() as u32, path.len() as u32,
            buf.as_mut_ptr() as u32, buf.len() as u32,
            &mut nread as *mut u32 as u32,
        )
    };
    if errno != 0 {
        eprintln!("ls: {}: errno {}", path, errno);
        std::process::exit(1);
    }
    let mut offset = 0usize;
    while offset + 12 <= nread as usize {
        let kind = buf[offset];
        // skip reserved byte
        let name_len = u16::from_le_bytes([buf[offset + 2], buf[offset + 3]]) as usize;
        let size = u64::from_le_bytes([
            buf[offset + 4], buf[offset + 5], buf[offset + 6], buf[offset + 7],
            buf[offset + 8], buf[offset + 9], buf[offset + 10], buf[offset + 11],
        ]);
        offset += 12;
        if offset + name_len > nread as usize { break; }
        let name = match std::str::from_utf8(&buf[offset..offset + name_len]) {
            Ok(s) => s,
            Err(_) => "?",
        };
        offset += name_len;
        let kind_str = match kind { 0 => "REG", 1 => "DIR", 2 => "DEV", _ => "???" };
        let mark = match kind { 1 => "/", 2 => "@", _ => "" };
        println!("{} {:>8} {}{}", kind_str, size, name, mark);
    }
}
```

- [ ] **Step 2.7: Create `user/shell/`**

```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && mkdir -p user/shell/src'
```

`user/shell/Cargo.toml`:
```toml
[package]
name = "shell"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "shell"
path = "src/main.rs"
```

`user/shell/src/main.rs`:
```rust
use std::fs;
use std::sync::Mutex;

static CWD: Mutex<String> = Mutex::new(String::new());

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn exec(
        path_ptr: u32, path_len: u32,
        argv_ptr: u32, argv_len: u32,
        exit_code_ptr: u32,
    ) -> i32;
}

fn main() {
    *CWD.lock().unwrap() = "/".to_string();
    if let Ok(script) = fs::read_to_string("/etc/init.sh") {
        for line in script.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            run_command(line);
        }
        println!("shell: init.sh complete");
    } else {
        println!("shell: /etc/init.sh not found");
    }
    // Interactive loop (not reached in make run-test; QEMU manual only).
    loop {
        print_prompt();
        match read_line() {
            Some(line) => {
                let line = line.trim();
                if line == "exit" { return; }
                if line.is_empty() { continue; }
                run_command(line);
            }
            None => return,
        }
    }
}

fn print_prompt() {
    let cwd = CWD.lock().unwrap().clone();
    print!("ruos:{}$ ", cwd);
    use std::io::Write;
    let _ = std::io::stdout().flush();
}

fn read_line() -> Option<String> {
    use std::io::Read;
    let mut buf = String::new();
    loop {
        let mut byte = [0u8; 1];
        match std::io::stdin().read(&mut byte) {
            Ok(0) => return if buf.is_empty() { None } else { Some(buf) },
            Ok(_) => {
                let c = byte[0];
                if c == b'\n' || c == b'\r' {
                    println!();
                    return Some(buf);
                }
                if c == 8 || c == 127 { // backspace
                    if !buf.is_empty() {
                        buf.pop();
                        print!("\x08 \x08");
                        use std::io::Write;
                        let _ = std::io::stdout().flush();
                    }
                    continue;
                }
                buf.push(c as char);
                let mut tmp = [0u8; 4];
                let s = (c as char).encode_utf8(&mut tmp);
                print!("{}", s);
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            Err(_) => return None,
        }
    }
}

fn run_command(line: &str) {
    let argv: Vec<&str> = line.split_whitespace().collect();
    if argv.is_empty() { return; }
    match argv[0] {
        "cd"   => builtin_cd(&argv),
        "pwd"  => builtin_pwd(),
        "exit" => std::process::exit(0),
        "help" => builtin_help(),
        cmd    => { let _ = exec_external(cmd, &argv); }
    }
}

fn builtin_pwd() {
    println!("{}", CWD.lock().unwrap());
}

fn builtin_cd(argv: &[&str]) {
    let target = argv.get(1).copied().unwrap_or("/");
    let mut cwd = CWD.lock().unwrap();
    let new = if target.starts_with('/') {
        target.to_string()
    } else if target == "." {
        cwd.clone()
    } else if target == ".." {
        let mut s = cwd.clone();
        if s.len() > 1 {
            if let Some(idx) = s.rfind('/') {
                s.truncate(idx.max(1));
            }
        }
        s
    } else {
        let mut s = cwd.clone();
        if !s.ends_with('/') { s.push('/'); }
        s.push_str(target);
        s
    };
    *cwd = new;
}

fn builtin_help() {
    println!("ruos shell builtins: cd <path>, pwd, exit, help");
    println!("external: try 'ls /bin' to list available .wasm");
}

fn exec_external(cmd: &str, argv: &[&str]) -> i32 {
    let candidates = if cmd.contains('/') {
        vec![cmd.to_string()]
    } else {
        vec![format!("/bin/{}.wasm", cmd)]
    };
    for path in &candidates {
        if let Some(code) = try_exec(path, argv) {
            return code;
        }
    }
    eprintln!("shell: {}: not found", cmd);
    127
}

fn try_exec(path: &str, argv: &[&str]) -> Option<i32> {
    // Encode argv: [u32 count][N x (u32 offset, u32 length)][bytes]
    let count = argv.len() as u32;
    let table_size = 4 + (argv.len() * 8);
    let mut blob: Vec<u8> = Vec::with_capacity(table_size + argv.iter().map(|s| s.len()).sum::<usize>());
    blob.extend_from_slice(&count.to_le_bytes());
    let mut data_offset = table_size as u32;
    for s in argv {
        blob.extend_from_slice(&data_offset.to_le_bytes());
        blob.extend_from_slice(&(s.len() as u32).to_le_bytes());
        data_offset += s.len() as u32;
    }
    for s in argv {
        blob.extend_from_slice(s.as_bytes());
    }
    let path_bytes = path.as_bytes();
    let mut exit_code: i32 = 0;
    let errno = unsafe {
        exec(
            path_bytes.as_ptr() as u32, path_bytes.len() as u32,
            blob.as_ptr() as u32, blob.len() as u32,
            &mut exit_code as *mut i32 as u32,
        )
    };
    if errno == 0 { Some(exit_code) } else { None }
}
```

- [ ] **Step 2.8: Create `user-bin/init.sh`**

Create `user-bin/init.sh`:
```
# ruos boot script — runs first by /bin/shell.wasm
echo hello from shell.wasm
pwd
ls /bin
echo init.sh end
```

(That's a literal text file, no shebang, no chmod needed.)

- [ ] **Step 2.9: Update `limine.conf`**

Add inside the `/ruos` entry, alongside the existing `init.wasm`/`server.wasm`/`client.wasm` module declarations:

```
    module_path: boot():/etc/init.sh
    module_cmdline: /etc/init.sh
    module_path: boot():/bin/shell.wasm
    module_cmdline: /bin/shell.wasm
    module_path: boot():/bin/ls.wasm
    module_cmdline: /bin/ls.wasm
    module_path: boot():/bin/cat.wasm
    module_cmdline: /bin/cat.wasm
    module_path: boot():/bin/echo.wasm
    module_cmdline: /bin/echo.wasm
```

The `module_cmdline` path determines where each file gets mounted in tmpfs. shell.wasm reads `/etc/init.sh` via `fs::read_to_string`.

**Important**: the Limine ISO needs the files at `/etc/init.sh` and `/bin/*.wasm` paths in the boot media. The Makefile's iso staging step must `mkdir -p build/iso_root/etc build/iso_root/bin` and copy accordingly.

- [ ] **Step 2.10: Update `Makefile` build rules**

Replace the existing user-wasm machinery with rules for 4 wasms (keeping init/server/client too) and an init.sh copy step:

```makefile
USER_WASMS := \
	user-bin/init.wasm \
	user-bin/server.wasm \
	user-bin/client.wasm \
	user-bin/shell.wasm \
	user-bin/ls.wasm \
	user-bin/cat.wasm \
	user-bin/echo.wasm

user-bin/init.wasm: user/init/src/main.rs user/init/Cargo.toml user/Cargo.toml
	cd user && cargo build --target wasm32-wasip1 --release -p init
	cp user/target/wasm32-wasip1/release/init.wasm user-bin/init.wasm

user-bin/server.wasm: user/server/src/main.rs user/server/Cargo.toml user/Cargo.toml
	cd user && cargo build --target wasm32-wasip1 --release -p server
	cp user/target/wasm32-wasip1/release/server.wasm user-bin/server.wasm

user-bin/client.wasm: user/client/src/main.rs user/client/Cargo.toml user/Cargo.toml
	cd user && cargo build --target wasm32-wasip1 --release -p client
	cp user/target/wasm32-wasip1/release/client.wasm user-bin/client.wasm

user-bin/shell.wasm: user/shell/src/main.rs user/shell/Cargo.toml user/Cargo.toml
	cd user && cargo build --target wasm32-wasip1 --release -p shell
	cp user/target/wasm32-wasip1/release/shell.wasm user-bin/shell.wasm

user-bin/ls.wasm: user/ls/src/main.rs user/ls/Cargo.toml user/Cargo.toml
	cd user && cargo build --target wasm32-wasip1 --release -p ls
	cp user/target/wasm32-wasip1/release/ls.wasm user-bin/ls.wasm

user-bin/cat.wasm: user/cat/src/main.rs user/cat/Cargo.toml user/Cargo.toml
	cd user && cargo build --target wasm32-wasip1 --release -p cat
	cp user/target/wasm32-wasip1/release/cat.wasm user-bin/cat.wasm

user-bin/echo.wasm: user/echo/src/main.rs user/echo/Cargo.toml user/Cargo.toml
	cd user && cargo build --target wasm32-wasip1 --release -p echo
	cp user/target/wasm32-wasip1/release/echo.wasm user-bin/echo.wasm

.PHONY: user-wasm
user-wasm: $(USER_WASMS)
```

Update the ISO target to depend on the wasms AND copy init.sh + create dirs:

```makefile
$(ISO): $(KERNEL_ELF) $(LIMINE_DEPS) limine.conf $(USER_WASMS) user-bin/init.sh
	rm -rf build/iso_root
	mkdir -p build/iso_root/boot build/iso_root/etc build/iso_root/bin
	cp $(KERNEL_ELF) build/iso_root/boot/kernel.elf
	cp limine.conf build/iso_root/
	cp user-bin/init.wasm build/iso_root/
	cp user-bin/server.wasm build/iso_root/
	cp user-bin/client.wasm build/iso_root/
	cp user-bin/shell.wasm build/iso_root/bin/
	cp user-bin/ls.wasm build/iso_root/bin/
	cp user-bin/cat.wasm build/iso_root/bin/
	cp user-bin/echo.wasm build/iso_root/bin/
	cp user-bin/init.sh build/iso_root/etc/
	# ... existing limine binary copies + xorriso + bios-install ...
```

(Preserve the existing limine + xorriso steps after the copies.)

- [ ] **Step 2.11: Spawn shell wasm_task in executor**

In `kernel/src/executor/mod.rs`, find the `run()` closure where wasm_task("/init.wasm") etc. are spawned. Add a new spawn alongside them:

```rust
        spawner.spawn(wasm_task("/bin/shell.wasm")).unwrap();
```

(Don't drop the existing spawns of init/server/client yet — Task 3 decides their fate. For now keep everything to avoid losing Step 10 demo coverage.)

If pool_size on `wasm_task` is 3, bump it to handle 4 spawns:

```rust
#[embassy_executor::task(pool_size = 4)]
async fn wasm_task(path: &'static str) { ... }
```

Actually you may need to bump to 5 or 6 (init/server/client/shell + child fibers spawned via ruos_exec). Bump to 8 to be safe:

```rust
#[embassy_executor::task(pool_size = 8)]
```

Wait — children spawned via `ruos_exec` are NOT embassy tasks. They run inside the parent task's await loop. So pool_size = 4 is fine for 4 concurrent top-level wasm tasks.

- [ ] **Step 2.12: Build all + test**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make user-wasm 2>&1 | tail -10'
```

Expected: 4 new wasm files in user-bin/ (plus the 3 existing). Errors are typically:
- `unknown crate "ls"` in workspace — verify members in user/Cargo.toml.
- wasm32-wasip1 fmt issues — check args API usage.

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -10'
```

Kernel still builds clean.

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 60 make run-test 2>&1 | tail -30'
```

Expected serial:
```
ruos: mounted 8 boot modules    (init/server/client/init.sh/shell/ls/cat/echo)
... (init logs)
init.wasm: argv0=/init.wasm
... welcome banner ...
... init.wasm output
ruos: init.wasm exited cleanly
... server/client (real ping-pong)
hello from shell.wasm
/
REG     1234 echo.wasm
REG     5678 cat.wasm
REG     9012 ls.wasm
REG  1200000 shell.wasm
init.sh end
shell: init.sh complete
ruos: shell.wasm exited cleanly
```

If `ls /bin` fails with `errno N`, trace ruos_readdir host fn — make sure it correctly traps SuspendReason::ReadDir.

If shell.wasm doesn't run at all (no "hello from shell.wasm" line), check:
- `/etc/init.sh` is properly mounted in tmpfs (kprintln list of mounted modules at boot)
- `fs::read_to_string("/etc/init.sh")` works (try reading from /init.wasm path as a control)

If shell starts but hangs after init.sh (interactive loop), that's expected — `timeout 60` will kill QEMU once sentinel is found.

- [ ] **Step 2.13: Changelog + commit**

```markdown
# 82 — Shell + external tools (Step 11 Task 2)

**Data:** 2026-05-29

## Cosa

- 4 user crate nuovi: shell, ls, cat, echo (Rust → wasm32-wasip1).
- shell.wasm: legge /etc/init.sh, esegue righe non-vuote non-commented,
  print "shell: init.sh complete". Builtin: cd/pwd/exit/help.
  External: PATH lookup /bin/<cmd>.wasm + ruos_exec.
- ls.wasm: chiama ruos_readdir + decodifica dirent buffer.
- cat.wasm: std::fs::File::open + read_to_end + print.
- echo.wasm: print args joined by space.
- /etc/init.sh: 4 righe demo (echo, pwd, ls /bin, echo).
- limine.conf: +5 moduli (1 testo, 4 wasm).
- Makefile: build di 4 nuovi wasm + iso staging mkdir /etc /bin
  + copy.
- executor::run: spawn wasm_task("/bin/shell.wasm") accanto agli
  altri.
- HELLO → `shell: init.sh complete`.

## Perché

Secondo task dello Step 11. Userland reale: shell che esegue tool
.wasm coordinati via init.sh. ruos_exec validato end-to-end
(shell spawna echo/ls/cat).

## File toccati

- user/Cargo.toml
- user/shell/{Cargo.toml,src/main.rs} (nuovi)
- user/ls/{Cargo.toml,src/main.rs} (nuovi)
- user/cat/{Cargo.toml,src/main.rs} (nuovi)
- user/echo/{Cargo.toml,src/main.rs} (nuovi)
- user-bin/{shell,ls,cat,echo}.wasm (nuovi)
- user-bin/init.sh (nuovo)
- limine.conf
- Makefile
- kernel/src/executor/mod.rs
- CHANGELOG/82-26-05-29-shell-user-crates.md (nuovo)
```

Commit:
```bash
git add user/ user-bin/ limine.conf Makefile kernel/src/executor/mod.rs CHANGELOG/82-26-05-29-shell-user-crates.md
git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit -m "feat(rust): shell.wasm + ls/cat/echo + init.sh boot script

Four new user crates compiled to wasm32-wasip1:
  shell — reads /etc/init.sh, exec's commands via ruos_exec host fn,
          builtins cd/pwd/exit/help, PATH lookup /bin/<cmd>.wasm
  ls    — ruos_readdir host fn, decodes flat dirent buffer
  cat   — std::fs::File::open + read_to_end + print
  echo  — args.join(' ')

/etc/init.sh + /bin/shell.wasm + /bin/{ls,cat,echo}.wasm declared as
Limine modules; Makefile creates iso_root/etc + iso_root/bin and
copies. executor::run spawns shell.wasm alongside existing
init/server/client wasm tasks.

Sentinel: 'shell: init.sh complete' after shell.wasm finishes
running every non-comment line of init.sh.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Drop `kbd_echo_task` + final regression

**Files:**
- Modify: `kernel/src/executor/mod.rs` (drop `kbd_echo_task` def + spawn)
- Modify: `Makefile` (HELLO unchanged)

**Smoke contract:** unchanged from T2 (`shell: init.sh complete`). All previous wasm output still present.

This closes F7 of Step 10.5 followups (KbdReadChar race with kbd_echo_task). Shell now owns keyboard exclusively via FdEntry::Stdin (or ConsoleFile from VFS path — both safe since shell is the only reader).

- [ ] **Step 3.1: HELLO unchanged**

```makefile
HELLO := shell: init.sh complete
```

- [ ] **Step 3.2: Delete `kbd_echo_task` definition**

In `kernel/src/executor/mod.rs`, find:

```rust
#[embassy_executor::task]
async fn kbd_echo_task() {
    loop {
        let b = crate::keyboard::queue::read_char().await;
        kprintln!("ruos: kbd echo={:?}", b as char);
    }
}
```

Delete this whole block.

- [ ] **Step 3.3: Remove `kbd_echo_task` spawn**

In `kernel/src/executor/mod.rs::run()`, find `spawner.spawn(kbd_echo_task()).unwrap();`. Delete this line.

- [ ] **Step 3.4: Build + test**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -10'
```

Expect clean. Warning count may drop by 1 (kbd_echo_task no longer counted).

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 60 make run-test 2>&1 | tail -30'
```

Expected: same sentinel `shell: init.sh complete`, all wasm output intact, the line `ruos: kbd echo=...` from any prior session is now gone (which was only in QEMU interactive sessions anyway).

- [ ] **Step 3.5: Changelog + commit**

```markdown
# 83 — Drop kbd_echo_task (Step 11 Task 3 / F7 Step 10.5)

**Data:** 2026-05-29

## Cosa

- Eliminato `kbd_echo_task` (definition + spawn) da
  `kernel/src/executor/mod.rs`.
- shell.wasm è ora l'unico consumer della keyboard queue (via FD 0
  → SuspendReason::KbdReadChar oppure /dev/console via ConsoleFile).

## Perché

Fix di F7 dei followup Step 10.5: race tra `kbd_echo_task` e ogni
wasm che legge stdin via fd_read(0). Con shell come unico processo
interattivo post-boot, l'eco di tastiera per debug serve a niente.

## File toccati

- kernel/src/executor/mod.rs
- CHANGELOG/83-26-05-29-shell-drop-kbd-echo.md (nuovo)
```

Commit:
```bash
git add kernel/src/executor/mod.rs CHANGELOG/83-26-05-29-shell-drop-kbd-echo.md
git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit -m "feat(rust): drop kbd_echo_task; shell.wasm owns keyboard

Closes F7 of docs/followups/step-10-5.md (KbdReadChar race vs
kbd_echo_task). Shell.wasm is now the only keyboard consumer
post-boot; the kernel-side debug echo task is removed.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Self-review (controller)

**Spec coverage:**
| Spec requirement | Implemented by |
|---|---|
| ruos_exec host fn | Task 1 |
| ruos_readdir host fn | Task 1 |
| SuspendReason::Exec / ReadDir + Fiber dispatch | Task 1 |
| Fiber::set_args + lifecycle::args_* real | Task 1 |
| shell.wasm (cd/pwd/exit/help builtins + exec_external) | Task 2 |
| ls.wasm / cat.wasm / echo.wasm | Task 2 |
| /etc/init.sh boot script + 5 Limine modules | Task 2 |
| executor spawns shell wasm_task | Task 2 |
| Drop kbd_echo_task (F7) | Task 3 |

**Type consistency:** `SuspendReason::Exec`, `SuspendReason::ReadDir`, `Fiber::set_args`, dirent format (kind u8 + reserved u8 + name_len u16 + size u64 + name bytes), argv encoding (count u32 + [offset u32, length u32] x N + bytes).

**Open risks:**
- `crate::wasm::read_all` visibility: may need promotion to `pub(crate)`.
- The `host_error()` clone pattern from Step 10.5 requires `SuspendReason: Clone`. Already derived (T1 step 1.3 adds new variants — verify they all have `Clone`-compatible field types).
- Limine `module_path` ordering: Limine 11.4.1 mounts modules in the order declared in `limine.conf`. /etc/init.sh must come before shell.wasm tries to read it. Modules are all mounted at boot before executor::run, so ordering doesn't matter — but verify.

---

## After all tasks complete

1. `make build` clean.
2. `make run-test` PASS (sentinel `shell: init.sh complete`).
3. Optional: manual VBox smoke — verify interactive prompt works (type `ls /`, see output; type `cat /etc/init.sh`, see script; type `exit`, shell terminates).
4. Final whole-implementation review (superpowers:code-reviewer agent).
5. Non-blocking findings → `docs/followups/step-11.md`.
6. Merge `feature/step-11-shell` → `main` no-ff, push, delete branch.
