# WASIX Bootstrap Implementation Plan (Step 10)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring up wasmi-based WASIX runtime inside ruos kernel — load `.wasm` modules via Limine, expose ~25 host functions (lifecycle, fd, path, clock, random, sockets), drive smoltcp loopback TCP. Final smoke: three demo `.wasm` running concurrently as embassy tasks, exchanging ping/pong over `127.0.0.1:8080`.

**Architecture:** `wasmi` 0.36 (pure Rust, no_std interpreter) hosted in `kernel/src/wasm/`. Each `.wasm` instance runs in its own `#[embassy_executor::task]`. Host fns are sync; async I/O (sockets, console) is bridged via `embassy_futures::block_on` invoked from inside the task. Network stack = `smoltcp` 0.11 with Loopback device + `net_poll_task` ticking every 10ms. WASM binaries loaded as **Limine modules** declared in `limine.conf`, mounted into tmpfs at boot via a new `kernel/src/modules.rs`.

**Tech Stack:** Rust nightly-2026-05-26, `no_std` + `alloc`, target `x86_64-unknown-none`. Demo `.wasm` built from `user/` workspace targeting `wasm32-wasip1`. New deps: `wasmi = "0.36"`, `smoltcp = "0.11"`, `embassy-futures = "0.1"`.

**Spec:** `docs/superpowers/specs/2026-05-28-rust-wasix-bootstrap-design.md`

**Branch:** `feature/wasix-bootstrap` (already created)

**Build host:** WSL Ubuntu, all commands via:
```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && <cmd>'
```

**Test model:** kernel-TDD — `make run-test` boots QEMU headless with serial → stdout, greps `HELLO` sentinel. Per task: bump HELLO → see FAIL → implement → see PASS → commit.

**Changelog rule:** Spec is `CHANGELOG/59`. This plan = `CHANGELOG/60`. Implementer tasks start at `61` (Task 1) through `66` (Task 6).

**Git identity:** `g.desolda <g.desolda@gmail.com>` via `git -c user.name=... -c user.email=...`. Co-author trailer mandatory:
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

**WASIX target requirement:** demo `.wasm` files are compiled with target `wasm32-wasip1`. This requires:
```
rustup target add wasm32-wasip1 --toolchain nightly-2026-05-26
```
Install in WSL before Task 2 if not already.

**Pre-flight: install wasm32-wasip1 target.** Run once:
```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && rustup target add wasm32-wasip1 --toolchain nightly-2026-05-26'
```

---

## File Structure

**New top-level (root):**
- `user/` — workspace separato per demo `.wasm`
  - `user/Cargo.toml` — workspace manifest
  - `user/init/{Cargo.toml,src/main.rs}` — welcome banner + crescente smoke
  - `user/server/{Cargo.toml,src/main.rs}` — TCP echo server (Task 6)
  - `user/client/{Cargo.toml,src/main.rs}` — TCP client (Task 6)
- `user-bin/` — build output `.wasm`, committed (alternativa: generato in `make iso`; partire committato)

**New kernel files:**
- `kernel/src/modules.rs` — Limine module enumeration + VFS mount
- `kernel/src/wasm/mod.rs` — Runtime wrapper
- `kernel/src/wasm/state.rs` — `RuntimeState` (per-instance: fd table, sock table, args, env, exit code)
- `kernel/src/wasm/host/mod.rs` — linker setup, import dispatch namespace
- `kernel/src/wasm/host/lifecycle.rs` — args/environ/proc_exit
- `kernel/src/wasm/host/fd.rs` — fd_*
- `kernel/src/wasm/host/path.rs` — path_*
- `kernel/src/wasm/host/clock.rs` — clock_time_get/clock_res_get
- `kernel/src/wasm/host/random.rs` — random_get (xorshift)
- `kernel/src/wasm/host/sock.rs` — sock_*
- `kernel/src/net/mod.rs` — Interface + SocketSet globali, `poll()`, `net_poll_task`
- `kernel/src/net/loopback.rs` — smoltcp `Loopback` wrapper
- `kernel/src/net/sockets.rs` — `SocketPool` (kernel-side FD↔SocketHandle map) + async accept/connect/recv/send

**Files modified:**
- `kernel/Cargo.toml` — `+wasmi`, `+smoltcp`, `+embassy-futures`
- `kernel/src/main.rs` — `mod modules; mod wasm; mod net;` + init calls
- `kernel/src/executor/mod.rs` — spawn `net_poll_task` + `wasm_task` × 3
- `limine.conf` — `module_path` × 3 + `module_cmdline`
- `Makefile` — `user-wasm` target, ISO include modules, `HELLO` sentinel per task

**Per-task HELLO progression:**
| Task | HELLO before | HELLO after |
|------|-------------|-------------|
| 1 | `ruos: async tick=2` | `ruos: mounted 1 boot modules` |
| 2 | `ruos: mounted 1 boot modules` | `ruos: init.wasm exited cleanly` |
| 3 | `ruos: init.wasm exited cleanly` | `ruos: init.wasm: vfs smoke ok` |
| 4 | `ruos: init.wasm: vfs smoke ok` | `ruos: init.wasm: clock_rand ok` |
| 5 | `ruos: init.wasm: clock_rand ok` | `ruos: net init ok addr=127.0.0.1/8` |
| 6 | `ruos: net init ok addr=127.0.0.1/8` | `ruos: client.wasm: rx='pong'` |

---

## Task 1: Limine modules → VFS mount

**Files:**
- Create: `kernel/src/modules.rs`
- Create: `user-bin/init.wasm` (dummy 1-byte placeholder for this task; Task 2 replaces with real content)
- Modify: `kernel/src/main.rs` (add `mod modules;`, call `modules::mount_all()` after `vfs::init`)
- Modify: `limine.conf` (declare module_path)
- Modify: `Makefile` (include `user-bin/init.wasm` in ISO; HELLO sentinel)

**What we're building:** infrastructure to load arbitrary `.wasm` files (or any boot module) into the VFS via Limine's `ModuleRequest`. End-state: kernel iterates Limine-loaded modules and copies each into tmpfs at the path declared in `limine.conf`'s `module_cmdline`. Boot log proves the count.

**Smoke contract:** `ruos: mounted 1 boot modules`

- [ ] **Step 1.1: Set the failing test sentinel**

Edit `Makefile`. Find:
```makefile
HELLO := ruos: async tick=2
```
Replace with:
```makefile
HELLO := ruos: mounted 1 boot modules
```

- [ ] **Step 1.2: Create a placeholder `.wasm`**

Create the `user-bin/` directory and a 1-byte placeholder so the build pipeline doesn't break before Task 2 replaces it with real bytecode:

```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && mkdir -p user-bin && printf "\x00" > user-bin/init.wasm'
```

It's a 1-byte file (`\x00`). Wasmi will reject it as invalid; that's fine for Task 1 — we don't load it into wasmi yet, just into the VFS.

- [ ] **Step 1.3: Update `limine.conf`**

Open `limine.conf`. The current content has a single `module_path` *missing entirely* (we have only kernel). Add inside the `/ruos` entry, after `kernel_path`:

```
    module_path: boot():/init.wasm
    module_cmdline: /init.wasm
```

So `limine.conf` becomes (approximately):
```
timeout: 0
default_entry: 1

/ruos
    protocol: limine
    kernel_path: boot():/boot/kernel.elf
    module_path: boot():/init.wasm
    module_cmdline: /init.wasm
```

(Verify by reading the current `limine.conf` first; preserve any other lines.)

- [ ] **Step 1.4: Update the `Makefile` to include the module in the ISO**

The current `Makefile` has an `iso_root` staging step that copies `kernel.elf` + limine binaries. Find that section. Add a line that copies `user-bin/init.wasm` into the ISO root.

Edit `Makefile`, find the line that copies the kernel into `build/iso_root/boot/`:

```makefile
$(ISO): $(KERNEL_ELF) $(LIMINE_DEPS) limine.conf
	rm -rf build/iso_root
	mkdir -p build/iso_root/boot
	cp $(KERNEL_ELF) build/iso_root/boot/kernel.elf
	cp limine.conf build/iso_root/
	# ... limine binaries ...
```

After `cp limine.conf build/iso_root/`, add:

```makefile
	cp user-bin/init.wasm build/iso_root/
```

(The path inside the ISO is just `/init.wasm` because that's what `limine.conf`'s `boot():/init.wasm` resolves to.)

- [ ] **Step 1.5: Run the test to verify it fails**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 30 make run-test 2>&1 | tail -20'
```

Expected: error — grep can't find `ruos: mounted 1 boot modules`. The kernel still only emits `ruos: async tick=N`.

- [ ] **Step 1.6: Create `kernel/src/modules.rs`**

```rust
//! Limine boot modules → VFS mount.
//!
//! At boot, Limine loads N modules declared in `limine.conf`. Each
//! module has a virtual path (its `module_cmdline`) and an in-RAM
//! buffer (mapped in HHDM). `mount_all()` copies each module into
//! the existing tmpfs at its declared path, so userspace .wasm files
//! become regular VFS entries (`/init.wasm`, `/server.wasm`, ...).

use crate::{kprintln, vfs};
use crate::vfs::{OpenFlags, Whence};
use limine::request::ModuleRequest;
use limine::BaseRevision;

#[used]
#[link_section = ".requests"]
static MODULES: ModuleRequest = ModuleRequest::new();

/// Iterate Limine modules, create+write each in tmpfs. Returns count
/// mounted. Panics if the request response is missing (Limine always
/// provides one even if no modules are declared).
pub fn mount_all() -> usize {
    let Some(resp) = MODULES.get_response() else {
        kprintln!("ruos: modules: no Limine response");
        return 0;
    };
    let mods = resp.modules();
    for m in mods {
        // SAFETY: Limine guarantees the module's address+size point to
        // a valid HHDM-mapped buffer for the lifetime of the kernel.
        let bytes: &[u8] = unsafe {
            core::slice::from_raw_parts(m.addr(), m.size() as usize)
        };
        // module_cmdline is the virtual path declared in limine.conf
        // (e.g. "/init.wasm"). Decode as UTF-8; treat invalid as ASCII.
        let cmdline = core::str::from_utf8(m.cmdline()).unwrap_or("/?");
        match install(cmdline, bytes) {
            Ok(()) => {
                kprintln!("ruos: module mounted at {} ({} bytes)", cmdline, bytes.len());
            }
            Err(e) => {
                kprintln!("ruos: module install fail {}: {:?}", cmdline, e);
            }
        }
    }
    kprintln!("ruos: mounted {} boot modules", mods.len());
    mods.len()
}

fn install(path: &str, bytes: &[u8]) -> Result<(), vfs::VfsError> {
    vfs::block_on(async {
        vfs::create(path).await?;
        let fd = vfs::open(path, OpenFlags::WRONLY).await?;
        vfs::write(fd, bytes).await?;
        vfs::close(fd).await?;
        Ok(())
    })
}
```

Note: API names `vfs::create`, `vfs::open`, `vfs::write`, `vfs::close`, `vfs::block_on`, `vfs::OpenFlags::WRONLY` must match the existing VFS module. If the existing names differ, adapt — but they were established in Step 7 (see `kernel/src/vfs/mod.rs`).

For `limine::request::ModuleRequest` and `Module::addr/size/cmdline`: the limine crate 0.6.3 exposes these. Field/method names may be `.address()`/`.size()`/`.cmdline()` or `.addr`/`.size`/`.cmdline` (public field) — check via `cargo doc --open` or the source under `kernel/src/main.rs`'s existing `FramebufferRequest` pattern for the right call style.

- [ ] **Step 1.7: Wire it in `main.rs`**

Edit `kernel/src/main.rs`. Add to the module declarations near the top (alongside `mod vfs;`, `mod executor;`):

```rust
mod modules;
```

Then in `kmain`, find the line `match vfs::init() { ... }`. After the VFS init has succeeded (and after the existing `vfs smoke ok` block), add:

```rust
    modules::mount_all();
```

The placement is: VFS up first, then mount modules into it.

- [ ] **Step 1.8: Build clean**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -20'
```

Expected: `Finished` line. New warnings allowed: at most 1-2 (e.g., `MODULES` unused if the wiring slipped).

If the build fails on the `Module::addr/size/cmdline` access pattern, the most common alternative is field-style:
```rust
let bytes = unsafe { core::slice::from_raw_parts(m.addr, m.size as usize) };
let cmdline = core::str::from_utf8(m.cmdline).unwrap_or("/?");
```

- [ ] **Step 1.9: Run the test to verify it passes**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 30 make run-test 2>&1 | tail -30'
```

Expected serial tail (cropped):
```
ruos: vfs init ok mounts=1
ruos: vfs smoke ok n=3 buf=[abc]
ruos: module mounted at /init.wasm (1 bytes)
ruos: mounted 1 boot modules
ruos: fb attached
...
ruos: executor up
ruos: async tick=0
make: *** [Makefile:NN: run-test] Terminated
```

The sentinel is `ruos: mounted 1 boot modules` (Makefile's HELLO).

- [ ] **Step 1.10: Changelog**

Create `CHANGELOG/61-26-05-28-wasix-limine-modules.md`:

```markdown
# 61 — Limine modules → VFS mount (Step 10 Task 1)

**Data:** 2026-05-28

## Cosa

- `limine.conf` dichiara `module_path: boot():/init.wasm` con
  `module_cmdline: /init.wasm`.
- `Makefile` include `user-bin/init.wasm` nel root dell'ISO.
- `kernel/src/modules.rs` (nuovo): `ModuleRequest` static, `mount_all()`
  itera i moduli e copia ciascuno in tmpfs al `module_cmdline` come path.
- `kmain` chiama `modules::mount_all()` dopo `vfs::init`.
- `user-bin/init.wasm` placeholder 1 byte; Task 2 lo sostituisce con
  bytecode reale.

## Perché

Primo task dello Step 10 (WASIX bootstrap). Standalone-loadable
boot modules sbloccano i prossimi 5 task: i `.wasm` arrivano al
kernel come file VFS, niente embedding.

## File toccati

- limine.conf
- Makefile
- kernel/src/modules.rs (nuovo)
- kernel/src/main.rs
- user-bin/init.wasm (placeholder 1 byte)
- CHANGELOG/61-26-05-28-wasix-limine-modules.md (nuovo)
```

- [ ] **Step 1.11: Commit**

```bash
git add limine.conf Makefile kernel/src/modules.rs kernel/src/main.rs user-bin/init.wasm CHANGELOG/61-26-05-28-wasix-limine-modules.md
git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit -m "feat(rust): Limine boot modules → VFS mount

Module file (e.g. /init.wasm) declared in limine.conf via
module_path/module_cmdline pair. Kernel iterates ModuleRequest at
boot and copies each module's bytes into tmpfs at its declared path,
becoming a regular VFS entry.

user-bin/init.wasm is a 1-byte placeholder; subsequent tasks replace
it with a real WASIX-targeted Rust binary.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: wasmi runtime + lifecycle + `fd_write` to console

**Files:**
- Create: `user/Cargo.toml`, `user/init/{Cargo.toml,src/main.rs}`
- Modify: `user-bin/init.wasm` (replace placeholder with real bytecode)
- Modify: `kernel/Cargo.toml` (add `wasmi`, `embassy-futures`)
- Create: `kernel/src/wasm/{mod,state}.rs`
- Create: `kernel/src/wasm/host/{mod,lifecycle,fd}.rs`
- Modify: `kernel/src/main.rs` (`mod wasm;`)
- Modify: `kernel/src/executor/mod.rs` (spawn `wasm_task("/init.wasm")`)
- Modify: `Makefile` (`user-wasm` build target + HELLO)

**Smoke contract:** `ruos: init.wasm exited cleanly`

The welcome banner is the user-visible content; the sentinel is the kernel-side log emitted after the wasm task catches `WasiTrap::Exit(0)`.

- [ ] **Step 2.1: Set the failing test sentinel**

Edit `Makefile`:
```makefile
HELLO := ruos: mounted 1 boot modules
```
to:
```makefile
HELLO := ruos: init.wasm exited cleanly
```

- [ ] **Step 2.2: Run the test to verify it fails**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 30 make run-test 2>&1 | tail -10'
```

Expected: error — sentinel not emitted yet.

- [ ] **Step 2.3: Add dependencies to `kernel/Cargo.toml`**

In the `[dependencies]` section, append:

```toml
wasmi = { version = "0.36", default-features = false }
embassy-futures = { version = "0.1", default-features = false }
```

- [ ] **Step 2.4: Build to verify deps resolve**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -15'
```

Expected: `Finished` line. wasmi and embassy-futures compile cleanly. If wasmi 0.36 fails (e.g., requires `nightly-2026-XX-XX` newer than ours), try `wasmi = "0.32"` as fallback.

- [ ] **Step 2.5: Create the `user/` workspace**

```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && mkdir -p user/init/src'
```

Create `user/Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["init"]

[profile.release]
opt-level = "s"
lto = true
panic = "abort"
strip = true
```

Create `user/init/Cargo.toml`:

```toml
[package]
name = "init"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "init"
path = "src/main.rs"
```

Create `user/init/src/main.rs`:

```rust
fn main() {
    println!("\x1b[1;32m╔══════════════════════════════════╗");
    println!("║         Welcome to ruos          ║");
    println!("║   wasm32-wasip1 / WASIX host     ║");
    println!("╚══════════════════════════════════╝\x1b[0m");
}
```

- [ ] **Step 2.6: Build the demo `.wasm`**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem/user && cargo build --target wasm32-wasip1 --release 2>&1 | tail -10'
```

Expected: `Finished release [optimized]` with the artifact at:
```
user/target/wasm32-wasip1/release/init.wasm
```

If the target is missing:
```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && rustup target add wasm32-wasip1 --toolchain nightly-2026-05-26'
```

- [ ] **Step 2.7: Add a `user-wasm` target in the root `Makefile`**

In the root `Makefile`, add (anywhere near the other build targets):

```makefile
USER_WASM := user-bin/init.wasm

$(USER_WASM): user/init/src/main.rs user/init/Cargo.toml user/Cargo.toml
	cd user && cargo build --target wasm32-wasip1 --release
	cp user/target/wasm32-wasip1/release/init.wasm user-bin/init.wasm

.PHONY: user-wasm
user-wasm: $(USER_WASM)
```

Make the `iso` target depend on `user-wasm` so a kernel rebuild also rebuilds the wasm:

```makefile
$(ISO): $(KERNEL_ELF) $(LIMINE_DEPS) limine.conf $(USER_WASM)
```

- [ ] **Step 2.8: Rebuild the wasm into `user-bin/`**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make user-wasm 2>&1 | tail -5 && ls -la user-bin/init.wasm'
```

Expected: a real `.wasm` file, several KB in size, replacing the 1-byte placeholder.

- [ ] **Step 2.9: Create `kernel/src/wasm/state.rs`**

```rust
//! Per-instance runtime state for a wasm task.

use alloc::vec::Vec;
use core::sync::atomic::AtomicI32;

pub struct RuntimeState {
    /// File descriptor table: index = FD, value = VFS Fd (or special).
    /// FDs 0/1/2 are reserved for stdin/stdout/stderr (Task 2 wires
    /// only stdout/stderr to the console; stdin lands in Task 3).
    pub fds: Vec<Option<FdEntry>>,
    /// args+env (filled by the spawner; demo uses empty).
    pub args: Vec<Vec<u8>>,
    pub env: Vec<Vec<u8>>,
    /// Exit code captured when wasm calls proc_exit (or hits an
    /// uncaught trap → mapped to a non-zero code).
    pub exit_code: AtomicI32,
}

pub enum FdEntry {
    /// Special: writes go to kprintln. FD 1 and 2 use this in Task 2.
    StdoutConsole,
    /// VFS-backed file (populated in Task 3).
    Vfs(crate::vfs::Fd),
}

impl RuntimeState {
    pub fn new() -> Self {
        let mut fds: Vec<Option<FdEntry>> = (0..16).map(|_| None).collect();
        fds[1] = Some(FdEntry::StdoutConsole);
        fds[2] = Some(FdEntry::StdoutConsole);
        Self {
            fds,
            args: Vec::new(),
            env: Vec::new(),
            exit_code: AtomicI32::new(0),
        }
    }
}
```

If `crate::vfs::Fd` doesn't exist as a `pub` type (it might be named differently), substitute the right name — check `kernel/src/vfs/fd.rs`.

- [ ] **Step 2.10: Create `kernel/src/wasm/host/lifecycle.rs`**

```rust
//! WASIX lifecycle host fns: args, environ, proc_exit.

use wasmi::{Caller, Linker, Memory, errors::Error};
use crate::wasm::state::RuntimeState;

/// proc_exit traps the wasm execution with a sentinel error. The
/// outer `Runtime::run` matches on it to capture the exit code.
#[derive(Debug)]
pub struct WasiTrap {
    pub exit_code: i32,
}
impl core::fmt::Display for WasiTrap {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "wasi proc_exit({})", self.exit_code)
    }
}
impl wasmi::core::HostError for WasiTrap {}

pub fn args_sizes_get(
    caller: Caller<'_, RuntimeState>,
    argc_ptr: i32,
    argv_buf_size_ptr: i32,
) -> Result<i32, Error> {
    let state = caller.data();
    let argc: u32 = state.args.len() as u32;
    let argv_buf: u32 = state.args.iter().map(|a| a.len() as u32 + 1).sum();
    let mem = wasm_memory(&caller)?;
    write_u32(&mem, &caller, argc_ptr, argc)?;
    write_u32(&mem, &caller, argv_buf_size_ptr, argv_buf)?;
    Ok(0) // errno = 0
}

pub fn args_get(
    _caller: Caller<'_, RuntimeState>,
    _argv_ptr: i32,
    _argv_buf_ptr: i32,
) -> Result<i32, Error> {
    // No args today (Task 2 demo doesn't read them). Implement when
    // a task needs them.
    Ok(0)
}

pub fn environ_sizes_get(
    caller: Caller<'_, RuntimeState>,
    environc_ptr: i32,
    environ_buf_size_ptr: i32,
) -> Result<i32, Error> {
    let mem = wasm_memory(&caller)?;
    write_u32(&mem, &caller, environc_ptr, 0)?;
    write_u32(&mem, &caller, environ_buf_size_ptr, 0)?;
    Ok(0)
}

pub fn environ_get(
    _caller: Caller<'_, RuntimeState>,
    _environ_ptr: i32,
    _environ_buf_ptr: i32,
) -> Result<i32, Error> {
    Ok(0)
}

pub fn proc_exit(
    caller: Caller<'_, RuntimeState>,
    code: i32,
) -> Result<(), Error> {
    caller.data().exit_code.store(code, core::sync::atomic::Ordering::SeqCst);
    Err(Error::host(WasiTrap { exit_code: code }))
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker
        .func_wrap("wasi_snapshot_preview1", "args_sizes_get", args_sizes_get)?
        .func_wrap("wasi_snapshot_preview1", "args_get", args_get)?
        .func_wrap("wasi_snapshot_preview1", "environ_sizes_get", environ_sizes_get)?
        .func_wrap("wasi_snapshot_preview1", "environ_get", environ_get)?
        .func_wrap("wasi_snapshot_preview1", "proc_exit", proc_exit)?;
    Ok(())
}

/// Helpers shared by host fns: read/write to wasm linear memory.
pub fn wasm_memory<'a>(caller: &'a Caller<'_, RuntimeState>) -> Result<Memory, Error> {
    caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| Error::host(WasiTrap { exit_code: -1 }))
}

pub fn write_u32(mem: &Memory, caller: &Caller<'_, RuntimeState>, ptr: i32, val: u32) -> Result<(), Error> {
    let bytes = val.to_le_bytes();
    mem.write(caller, ptr as usize, &bytes)
        .map_err(|_| Error::host(WasiTrap { exit_code: -1 }))
}

pub fn read_u32(mem: &Memory, caller: &Caller<'_, RuntimeState>, ptr: i32) -> Result<u32, Error> {
    let mut buf = [0u8; 4];
    mem.read(caller, ptr as usize, &mut buf)
        .map_err(|_| Error::host(WasiTrap { exit_code: -1 }))?;
    Ok(u32::from_le_bytes(buf))
}
```

Note: wasmi 0.36 API surface for `Caller`, `Memory`, `Linker`, `Error` may differ slightly. The pattern (link via `func_wrap`, get memory via `caller.get_export("memory")`, read/write via `Memory::read`/`write`) is consistent across 0.32-0.40 — adapt names if rustc complains.

- [ ] **Step 2.11: Create `kernel/src/wasm/host/fd.rs`**

```rust
//! WASIX file descriptor host fns. Task 2 stubs everything except
//! fd_write (routed to console for FD 1/2). VFS-backed paths in Task 3.

use wasmi::{Caller, Linker, errors::Error};
use crate::wasm::state::{RuntimeState, FdEntry};
use crate::wasm::host::lifecycle::{wasm_memory, read_u32, write_u32, WasiTrap};
use crate::kprintln;

/// fd_write(fd, iovs_ptr, iovs_len, nwritten_ptr) -> errno
pub fn fd_write(
    mut caller: Caller<'_, RuntimeState>,
    fd: i32,
    iovs_ptr: i32,
    iovs_len: i32,
    nwritten_ptr: i32,
) -> Result<i32, Error> {
    let mem = wasm_memory(&caller)?;
    let mut total: u32 = 0;
    // Walk N iovec structs in wasm memory: each iovec = (buf_ptr: u32, buf_len: u32) = 8 bytes
    for i in 0..iovs_len {
        let iov_at = iovs_ptr + i * 8;
        let buf_ptr = read_u32(&mem, &caller, iov_at)?;
        let buf_len = read_u32(&mem, &caller, iov_at + 4)?;
        if buf_len == 0 { continue; }
        // Read the bytes from wasm memory into a small kernel-side buffer.
        // Cap at 4 KiB per iov for safety in this minimal impl.
        const MAX: usize = 4096;
        let mut buf = [0u8; MAX];
        let n = (buf_len as usize).min(MAX);
        mem.read(&caller, buf_ptr as usize, &mut buf[..n])
            .map_err(|_| Error::host(WasiTrap { exit_code: -1 }))?;
        // Dispatch by FD type.
        match caller.data().fds.get(fd as usize).and_then(|x| x.as_ref()) {
            Some(FdEntry::StdoutConsole) => {
                // Decode as UTF-8 ignoring errors; write via kprint without newline.
                if let Ok(s) = core::str::from_utf8(&buf[..n]) {
                    // kprint! has no newline; print verbatim.
                    use core::fmt::Write;
                    let mut c = crate::console::CONSOLE.lock();
                    let _ = c.write_str(s);
                } else {
                    kprintln!("ruos: fd_write: non-utf8 ({} bytes)", n);
                }
            }
            Some(FdEntry::Vfs(_)) => {
                // Task 3 implements VFS-backed write; return EBADF for now.
                return Ok(8); // EBADF
            }
            None => return Ok(8), // EBADF
        }
        total += n as u32;
    }
    write_u32(&mem, &caller, nwritten_ptr, total)?;
    Ok(0)
}

/// fd_read stub for Task 2; Task 3 wires VFS read.
pub fn fd_read_stub(_: Caller<'_, RuntimeState>, _: i32, _: i32, _: i32, _: i32) -> Result<i32, Error> {
    Ok(8) // EBADF
}

/// fd_close stub for Task 2.
pub fn fd_close_stub(_: Caller<'_, RuntimeState>, _: i32) -> Result<i32, Error> {
    Ok(0)
}

/// fd_seek stub.
pub fn fd_seek_stub(_: Caller<'_, RuntimeState>, _: i32, _: i64, _: i32, _: i32) -> Result<i32, Error> {
    Ok(8) // EBADF
}

/// fd_fdstat_get stub. Returns a buffer of zeros (some libc impls
/// require any value to avoid panic in __wasi_init).
pub fn fd_fdstat_get(
    caller: Caller<'_, RuntimeState>,
    _fd: i32,
    stat_ptr: i32,
) -> Result<i32, Error> {
    // wasi_snapshot_preview1 fdstat is 24 bytes: u8 fs_filetype, padding,
    // u16 fs_flags, padding, u64 fs_rights_base, u64 fs_rights_inheriting.
    let mem = wasm_memory(&caller)?;
    let zeros = [0u8; 24];
    mem.write(&caller, stat_ptr as usize, &zeros)
        .map_err(|_| Error::host(WasiTrap { exit_code: -1 }))?;
    Ok(0)
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker
        .func_wrap("wasi_snapshot_preview1", "fd_write", fd_write)?
        .func_wrap("wasi_snapshot_preview1", "fd_read", fd_read_stub)?
        .func_wrap("wasi_snapshot_preview1", "fd_close", fd_close_stub)?
        .func_wrap("wasi_snapshot_preview1", "fd_seek", fd_seek_stub)?
        .func_wrap("wasi_snapshot_preview1", "fd_fdstat_get", fd_fdstat_get)?;
    Ok(())
}
```

Note: `crate::console::CONSOLE` must be `pub`; if not, expose a helper `pub fn write_str(s: &str)` in `kernel/src/console/mod.rs` and use that instead.

- [ ] **Step 2.12: Create `kernel/src/wasm/host/mod.rs`**

```rust
//! WASIX host functions namespace.

pub mod lifecycle;
pub mod fd;

use wasmi::{Linker, errors::Error};
use crate::wasm::state::RuntimeState;

/// Register all host fns into the given linker. Tasks 3-6 will extend
/// this with more modules (path, clock, random, sock).
pub fn install(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    lifecycle::link(linker)?;
    fd::link(linker)?;
    Ok(())
}
```

- [ ] **Step 2.13: Create `kernel/src/wasm/mod.rs`**

```rust
//! Wasmi runtime hosting layer for ruos.

pub mod state;
pub mod host;

use alloc::vec::Vec;
use wasmi::{Engine, Module, Store, Linker};
use crate::kprintln;
use crate::vfs;
use crate::wasm::state::RuntimeState;
use crate::wasm::host::lifecycle::WasiTrap;

pub struct Runtime {
    store: Store<RuntimeState>,
    instance: wasmi::Instance,
}

impl Runtime {
    pub fn new(bytes: &[u8]) -> Result<Self, wasmi::errors::Error> {
        let engine = Engine::default();
        let module = Module::new(&engine, bytes)?;
        let mut store: Store<RuntimeState> = Store::new(&engine, RuntimeState::new());
        let mut linker: Linker<RuntimeState> = Linker::new(&engine);
        host::install(&mut linker)?;
        let pre = linker.instantiate(&mut store, &module)?;
        let instance = pre.start(&mut store)?;
        Ok(Self { store, instance })
    }

    /// Run the `_start` export. Returns exit code (0 if clean,
    /// nonzero if proc_exit code or trap).
    pub fn run(&mut self) -> i32 {
        let start = match self.instance.get_typed_func::<(), ()>(&self.store, "_start") {
            Ok(f) => f,
            Err(e) => {
                kprintln!("ruos: wasm: no _start export: {}", e);
                return -1;
            }
        };
        match start.call(&mut self.store, ()) {
            Ok(()) => 0,
            Err(e) => {
                // Unwrap WasiTrap from the error chain if present.
                if let Some(t) = e.downcast_ref::<WasiTrap>() {
                    t.exit_code
                } else {
                    kprintln!("ruos: wasm trap: {}", e);
                    -1
                }
            }
        }
    }
}

/// Load and run a single wasm module from the VFS path. Called from
/// the `wasm_task` embassy task.
pub async fn run_at(path: &str) {
    let bytes = match read_all(path).await {
        Ok(b) => b,
        Err(e) => {
            kprintln!("ruos: wasm: read {} failed: {:?}", path, e);
            return;
        }
    };
    let mut rt = match Runtime::new(&bytes) {
        Ok(r) => r,
        Err(e) => {
            kprintln!("ruos: wasm: instantiate {} failed: {}", path, e);
            return;
        }
    };
    let code = rt.run();
    if code == 0 {
        kprintln!("ruos: {} exited cleanly", path);
    } else {
        kprintln!("ruos: {} exited code={}", path, code);
    }
}

async fn read_all(path: &str) -> Result<Vec<u8>, vfs::VfsError> {
    use vfs::{OpenFlags, Whence};
    let fd = vfs::open(path, OpenFlags::RDONLY).await?;
    // Seek to end to learn size.
    let end = vfs::seek(fd, 0, Whence::End).await? as usize;
    vfs::seek(fd, 0, Whence::Start).await?;
    let mut buf = alloc::vec![0u8; end];
    let mut read = 0;
    while read < end {
        let n = vfs::read(fd, &mut buf[read..]).await?;
        if n == 0 { break; }
        read += n;
    }
    vfs::close(fd).await?;
    Ok(buf)
}
```

`vfs::OpenFlags::RDONLY` and `Whence::Start`/`Whence::End` may have different exact names in the existing vfs module — check `kernel/src/vfs/mod.rs` for the correct ones.

- [ ] **Step 2.14: Spawn the wasm task from the executor**

Edit `kernel/src/executor/mod.rs`. Add a wasm_task function and spawn it.

After the existing `tick_task` and `kbd_echo_task` definitions, add:

```rust
#[embassy_executor::task(pool_size = 3)]
async fn wasm_task(path: &'static str) {
    crate::wasm::run_at(path).await;
}
```

In the `run()` function, after the existing two `spawner.spawn(...)` calls, add:

```rust
    spawner.spawn(wasm_task("/init.wasm")).unwrap();
```

- [ ] **Step 2.15: Register the wasm module in `main.rs`**

Edit `kernel/src/main.rs`. Add to the `mod` block:

```rust
mod wasm;
```

- [ ] **Step 2.16: Build**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -30'
```

Expected: compile succeeds. wasmi-specific API mismatches will surface here; iterate. Common adaptations:
- `wasmi::errors::Error` may be `wasmi::Error`.
- `Module::new(&engine, bytes)` may need `&engine` or `engine.clone()`.
- `Linker::new(&engine)` API may have shifted.
- `e.downcast_ref::<WasiTrap>()` may require accessing the inner `HostError` first via `e.source()`.

Treat the code above as a sketch; the implementer must adapt to the exact wasmi 0.36 API. Use `cargo doc --package wasmi --open` (locally on the WSL host) to inspect.

- [ ] **Step 2.17: Run the test to verify it passes**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 30 make run-test 2>&1 | tail -30'
```

Expected serial tail (cropped):
```
ruos: module mounted at /init.wasm (5234 bytes)
ruos: mounted 1 boot modules
... (other init logs) ...
ruos: executor up
ruos: async tick=0
\x1b[1;32m╔══════════════════════════════════╗
║         Welcome to ruos          ║
║   wasm32-wasip1 / WASIX host     ║
╚══════════════════════════════════╝\x1b[0m
ruos: /init.wasm exited cleanly
make: *** [Makefile:NN: run-test] Terminated
```

The banner is the wasm output (`println!` → libc `fd_write` → host `fd_write` → console). The sentinel is `ruos: /init.wasm exited cleanly`.

If you see `ruos: wasm: instantiate ... failed: missing import ...`, list what import is missing and add a stub for it under `kernel/src/wasm/host/`. Common ones: `fd_fdstat_set_flags`, `fd_prestat_get`, `clock_time_get` (we add this in Task 4 but might be called by `_start` initialization in libc — if so, add a stub that returns errno 0 with zeroed output).

**Important:** the actual Makefile HELLO sentinel is `ruos: init.wasm exited cleanly` (without leading `/`). Make sure the kprintln in `run_at` matches:

If `kprintln!("ruos: {} exited cleanly", path)` prints `ruos: /init.wasm exited cleanly`, the test will fail because grep looks for `ruos: init.wasm exited cleanly`. Either:
- Adjust the kprintln to strip the leading `/`: `kprintln!("ruos: {} exited cleanly", path.trim_start_matches('/'))`
- Or adjust the HELLO to include the `/`.

Pick the first — cleaner log style. Update the code in `wasm::run_at` from Step 2.13 accordingly.

- [ ] **Step 2.18: Changelog + commit**

Create `CHANGELOG/62-26-05-28-wasix-wasmi-up.md`:

```markdown
# 62 — wasmi runtime up + lifecycle + fd_write console (Step 10 Task 2)

**Data:** 2026-05-28

## Cosa

- Aggiunte deps `wasmi = "0.36"` + `embassy-futures = "0.1"`.
- Nuovo `kernel/src/wasm/` modulo: `Runtime` wrapper su wasmi
  `Engine`/`Module`/`Store`/`Instance`. `run()` chiama `_start` e
  cattura `WasiTrap::Exit(code)`.
- Nuovo `kernel/src/wasm/state.rs`: `RuntimeState` con tabella FD;
  FD 1/2 mappati a `StdoutConsole` (console direct).
- Nuovo `kernel/src/wasm/host/lifecycle.rs`: `args_*`, `environ_*`,
  `proc_exit` (5 host fns).
- Nuovo `kernel/src/wasm/host/fd.rs`: `fd_write` (verso console),
  stubs `fd_read`/`fd_close`/`fd_seek`/`fd_fdstat_get`.
- `executor::run` spawna `wasm_task("/init.wasm")` come 3° task.
- Nuovo `user/` workspace con crate `init` (welcome banner).
- `Makefile` target `user-wasm` builda `wasm32-wasip1` e copia in
  `user-bin/init.wasm`. ISO dipende da `user-wasm`.
- HELLO → `ruos: init.wasm exited cleanly`.

## Perché

Secondo task dello Step 10. Materializza il "wasm runs" end-to-end:
binario Rust reale compilato `wasm32-wasip1`, caricato via Limine
module, instanziato da wasmi, esegue, stampa welcome ANSI, termina
clean. Sblocca i 4 task successivi (host fns aggiuntive).

## File toccati

- kernel/Cargo.toml
- kernel/src/wasm/{mod,state}.rs (nuovi)
- kernel/src/wasm/host/{mod,lifecycle,fd}.rs (nuovi)
- kernel/src/main.rs
- kernel/src/executor/mod.rs
- Makefile
- user/Cargo.toml, user/init/{Cargo.toml,src/main.rs} (nuovi)
- user-bin/init.wasm (rigenerato, ora reale)
- CHANGELOG/62-26-05-28-wasix-wasmi-up.md (nuovo)
```

Then commit:

```bash
git add kernel/Cargo.toml kernel/Cargo.lock kernel/src/wasm/ kernel/src/main.rs kernel/src/executor/mod.rs Makefile user/ user-bin/init.wasm CHANGELOG/62-26-05-28-wasix-wasmi-up.md
git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit -m "feat(rust): wasmi runtime + WASIX lifecycle + fd_write to console

embassy task wraps a wasmi Engine/Module/Store/Instance per .wasm
module mounted in tmpfs. WasiTrap (HostError) propagates proc_exit's
code out of the _start call. FD 1/2 route to the console; later
tasks add VFS-backed read/write/seek and sockets.

Demo: user/init/ compiles to wasm32-wasip1, prints an ANSI welcome
banner via println!, and exits cleanly. Boot sentinel:
'ruos: init.wasm exited cleanly'.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: VFS-backed `fd_*` + `path_*` host fns + init.wasm VFS smoke

**Files:**
- Modify: `user/init/src/main.rs` (add VFS smoke after banner)
- Modify: `kernel/src/wasm/state.rs` (FdEntry::Vfs already declared; populate path)
- Modify: `kernel/src/wasm/host/fd.rs` (real impl of fd_read/fd_seek/fd_close)
- Create: `kernel/src/wasm/host/path.rs`
- Modify: `kernel/src/wasm/host/mod.rs` (link path module)
- Modify: `Makefile` (HELLO)

**What we're building:** make `path_open` resolve a wasm-side path (e.g. `/dev/null`) to a `vfs::Fd`, allocate a wasm FD pointing at it, then `fd_write`/`fd_read`/`fd_seek`/`fd_close` dispatch to the VFS for that FD.

**Smoke contract:** init.wasm successfully writes 10 bytes to `/dev/null`, closes it, and prints `init.wasm: vfs smoke ok` via fd_write to stdout. Sentinel = `ruos: init.wasm: vfs smoke ok`.

- [ ] **Step 3.1: Set the failing test sentinel**

Edit `Makefile`:
```makefile
HELLO := ruos: init.wasm exited cleanly
```
to:
```makefile
HELLO := ruos: init.wasm: vfs smoke ok
```

- [ ] **Step 3.2: Run to verify fail**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 30 make run-test 2>&1 | tail -10'
```

Expected: error (sentinel absent).

- [ ] **Step 3.3: Create `kernel/src/wasm/host/path.rs`**

```rust
//! WASIX path_* host fns. Resolve a wasm-side path to a VFS Fd,
//! allocate a wasm-side FD entry, return it to the wasm.

use wasmi::{Caller, Linker, errors::Error};
use alloc::string::String;
use crate::wasm::state::{RuntimeState, FdEntry};
use crate::wasm::host::lifecycle::{wasm_memory, write_u32, WasiTrap};
use crate::vfs::{self, OpenFlags};

/// path_open(dir_fd, dir_flags, path_ptr, path_len, oflags,
///           fs_rights_base, fs_rights_inheriting, fd_flags,
///           opened_fd_ptr) -> errno
///
/// Minimal impl ignores dir_fd/dir_flags/rights/fd_flags; the wasm-side
/// path is the entire absolute path. WASIX-as-used by Rust's wasi
/// libc passes paths relative to a preopened directory; for Task 3 we
/// treat any path as already absolute (the demo passes "/dev/null").
pub fn path_open(
    mut caller: Caller<'_, RuntimeState>,
    _dir_fd: i32,
    _dir_flags: i32,
    path_ptr: i32,
    path_len: i32,
    _oflags: i32,
    _fs_rights_base: i64,
    _fs_rights_inheriting: i64,
    _fd_flags: i32,
    opened_fd_ptr: i32,
) -> Result<i32, Error> {
    let mem = wasm_memory(&caller)?;
    let mut path_buf = alloc::vec![0u8; path_len as usize];
    mem.read(&caller, path_ptr as usize, &mut path_buf)
        .map_err(|_| Error::host(WasiTrap { exit_code: -1 }))?;
    let path = core::str::from_utf8(&path_buf)
        .map_err(|_| Error::host(WasiTrap { exit_code: -1 }))?;
    // Ensure leading '/'.
    let path: String = if path.starts_with('/') {
        String::from(path)
    } else {
        let mut p = String::from("/");
        p.push_str(path);
        p
    };
    // Synchronous: drive the VFS open via block_on (init-time pattern).
    // We use embassy-futures::block_on since we're inside an embassy task.
    let res: Result<vfs::Fd, vfs::VfsError> =
        embassy_futures::block_on(vfs::open(&path, OpenFlags::RDWR));
    let vfd = match res {
        Ok(f) => f,
        Err(_) => return Ok(44), // ENOENT (WASI errno; adjust if exact code differs)
    };
    // Find a free wasm-side FD slot starting from 3 (0/1/2 reserved).
    let state = caller.data_mut();
    let mut wfd: Option<usize> = None;
    for (i, slot) in state.fds.iter_mut().enumerate().skip(3) {
        if slot.is_none() {
            *slot = Some(FdEntry::Vfs(vfd));
            wfd = Some(i);
            break;
        }
    }
    let wfd = match wfd {
        Some(w) => w,
        None => {
            // Grow the table.
            state.fds.push(Some(FdEntry::Vfs(vfd)));
            state.fds.len() - 1
        }
    };
    write_u32(&mem, &caller, opened_fd_ptr, wfd as u32)?;
    Ok(0)
}

/// path_unlink_file stub: not needed for /dev/null write smoke.
pub fn path_unlink_file(_: Caller<'_, RuntimeState>, _: i32, _: i32, _: i32) -> Result<i32, Error> {
    Ok(58) // ENOSYS
}
pub fn path_create_directory(_: Caller<'_, RuntimeState>, _: i32, _: i32, _: i32) -> Result<i32, Error> {
    Ok(58)
}
pub fn path_remove_directory(_: Caller<'_, RuntimeState>, _: i32, _: i32, _: i32) -> Result<i32, Error> {
    Ok(58)
}
pub fn path_filestat_get(_: Caller<'_, RuntimeState>, _: i32, _: i32, _: i32, _: i32, _: i32) -> Result<i32, Error> {
    Ok(58)
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker
        .func_wrap("wasi_snapshot_preview1", "path_open", path_open)?
        .func_wrap("wasi_snapshot_preview1", "path_unlink_file", path_unlink_file)?
        .func_wrap("wasi_snapshot_preview1", "path_create_directory", path_create_directory)?
        .func_wrap("wasi_snapshot_preview1", "path_remove_directory", path_remove_directory)?
        .func_wrap("wasi_snapshot_preview1", "path_filestat_get", path_filestat_get)?;
    Ok(())
}
```

- [ ] **Step 3.4: Replace `fd_read_stub`/`fd_close_stub`/`fd_seek_stub` with real impls**

In `kernel/src/wasm/host/fd.rs`, replace the stubs and update the `link()` accordingly:

```rust
pub fn fd_read(
    caller: Caller<'_, RuntimeState>,
    fd: i32,
    iovs_ptr: i32,
    iovs_len: i32,
    nread_ptr: i32,
) -> Result<i32, Error> {
    let mem = wasm_memory(&caller)?;
    let entry = match caller.data().fds.get(fd as usize).and_then(|x| x.as_ref()) {
        Some(FdEntry::Vfs(vfd)) => *vfd,
        _ => return Ok(8), // EBADF — Task 4 wires FD 0 (stdin) to keyboard
    };
    let mut total: u32 = 0;
    for i in 0..iovs_len {
        let iov_at = iovs_ptr + i * 8;
        let buf_ptr = read_u32(&mem, &caller, iov_at)?;
        let buf_len = read_u32(&mem, &caller, iov_at + 4)?;
        if buf_len == 0 { continue; }
        const MAX: usize = 4096;
        let mut buf = [0u8; MAX];
        let n = (buf_len as usize).min(MAX);
        let read_n = embassy_futures::block_on(crate::vfs::read(entry, &mut buf[..n]))
            .map_err(|_| Error::host(WasiTrap { exit_code: -1 }))?;
        mem.write(&caller, buf_ptr as usize, &buf[..read_n])
            .map_err(|_| Error::host(WasiTrap { exit_code: -1 }))?;
        total += read_n as u32;
        if read_n < n { break; }
    }
    write_u32(&mem, &caller, nread_ptr, total)?;
    Ok(0)
}

pub fn fd_seek(
    caller: Caller<'_, RuntimeState>,
    fd: i32,
    offset: i64,
    whence: i32,
    newoffset_ptr: i32,
) -> Result<i32, Error> {
    let entry = match caller.data().fds.get(fd as usize).and_then(|x| x.as_ref()) {
        Some(FdEntry::Vfs(vfd)) => *vfd,
        _ => return Ok(8),
    };
    let w = match whence {
        0 => crate::vfs::Whence::Start,
        1 => crate::vfs::Whence::Cur,
        2 => crate::vfs::Whence::End,
        _ => return Ok(28), // EINVAL
    };
    let n = embassy_futures::block_on(crate::vfs::seek(entry, offset, w))
        .map_err(|_| Error::host(WasiTrap { exit_code: -1 }))?;
    let mem = wasm_memory(&caller)?;
    let bytes = (n as u64).to_le_bytes();
    mem.write(&caller, newoffset_ptr as usize, &bytes)
        .map_err(|_| Error::host(WasiTrap { exit_code: -1 }))?;
    Ok(0)
}

pub fn fd_close(
    mut caller: Caller<'_, RuntimeState>,
    fd: i32,
) -> Result<i32, Error> {
    let entry = match caller.data_mut().fds.get_mut(fd as usize).and_then(|x| x.take()) {
        Some(FdEntry::Vfs(vfd)) => vfd,
        Some(other) => {
            // Restore non-VFS entry (don't close stdin/stdout).
            caller.data_mut().fds[fd as usize] = Some(other);
            return Ok(0);
        }
        None => return Ok(8),
    };
    let _ = embassy_futures::block_on(crate::vfs::close(entry));
    Ok(0)
}
```

Update `fd::link()` to use `fd_read`, `fd_seek`, `fd_close` instead of the `_stub` versions.

Also extend `fd_write` (Step 2.11) to dispatch to VFS for `FdEntry::Vfs(_)`:

```rust
            Some(FdEntry::Vfs(vfd)) => {
                let written = embassy_futures::block_on(crate::vfs::write(*vfd, &buf[..n]))
                    .map_err(|_| Error::host(WasiTrap { exit_code: -1 }))?;
                total += written as u32;
            }
```

- [ ] **Step 3.5: Update `host/mod.rs` to link path**

```rust
pub mod lifecycle;
pub mod fd;
pub mod path;

use wasmi::{Linker, errors::Error};
use crate::wasm::state::RuntimeState;

pub fn install(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    lifecycle::link(linker)?;
    fd::link(linker)?;
    path::link(linker)?;
    Ok(())
}
```

- [ ] **Step 3.6: Extend `user/init/src/main.rs`**

Replace the file with:

```rust
use std::fs::OpenOptions;
use std::io::Write;

fn main() {
    println!("\x1b[1;32m╔══════════════════════════════════╗");
    println!("║         Welcome to ruos          ║");
    println!("║   wasm32-wasip1 / WASIX host     ║");
    println!("╚══════════════════════════════════╝\x1b[0m");

    // VFS smoke: open /dev/null, write 10 bytes, close.
    match OpenOptions::new().write(true).open("/dev/null") {
        Ok(mut f) => {
            match f.write_all(b"0123456789") {
                Ok(()) => println!("init.wasm: vfs smoke ok"),
                Err(e) => println!("init.wasm: vfs write fail: {}", e),
            }
        }
        Err(e) => println!("init.wasm: vfs open fail: {}", e),
    }
}
```

This produces the exact `init.wasm: vfs smoke ok` line via fd_write to stdout, which the kernel forwards to console (and the Makefile greps for it as part of `ruos: init.wasm: vfs smoke ok`).

Wait — there's a sentinel mismatch: stdout prints `init.wasm: vfs smoke ok`, kernel doesn't auto-prefix `ruos:`. Either:
- Change HELLO to grep `init.wasm: vfs smoke ok` (no `ruos:` prefix) — simpler.
- Or modify `fd_write` console-path to prepend `ruos: ` — clutters all wasm output.

Pick option 1. Update HELLO to `init.wasm: vfs smoke ok` (no `ruos:` prefix). Update Step 3.1 accordingly.

Also update the previous "ruos: {} exited cleanly" log: it still emits with prefix `ruos:`, only the wasm-output lines bypass the prefix. Both are visible in the serial dump.

- [ ] **Step 3.7: Rebuild wasm**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make user-wasm 2>&1 | tail -5'
```

- [ ] **Step 3.8: Build kernel**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -15'
```

Expected: `Finished`. The wasmi-vs-vfs integration adds path.rs and the VFS calls.

- [ ] **Step 3.9: Run the test to verify it passes**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 30 make run-test 2>&1 | tail -20'
```

Expected serial tail:
```
... banner ...
init.wasm: vfs smoke ok
ruos: init.wasm exited cleanly
```

HELLO grep `init.wasm: vfs smoke ok` matches.

If you see `init.wasm: vfs open fail: ...`, the `path_open` path translation or VFS open path is broken. Common pitfalls:
- `/dev/null` doesn't exist in tmpfs (Step 7 must have populated it; verify with the vfs smoke).
- `OpenFlags::RDWR` doesn't exist (try `OpenFlags::WRONLY` for write-only).
- The path string isn't null-terminated and `from_utf8` reads garbage.

- [ ] **Step 3.10: Changelog + commit**

`CHANGELOG/63-26-05-28-wasix-vfs-host-fns.md`:

```markdown
# 63 — VFS-backed fd_* + path_* host fns (Step 10 Task 3)

**Data:** 2026-05-28

## Cosa

- `kernel/src/wasm/host/path.rs` (nuovo): `path_open` risolve path
  wasm → VFS Fd, alloca slot nella tabella `RuntimeState.fds`. Stubs
  per `path_unlink_file`, `path_create_directory`, ecc.
- `kernel/src/wasm/host/fd.rs`: `fd_read`/`fd_seek`/`fd_close`
  reali (sostituiscono stubs Task 2). `fd_write` ora dispatcha anche
  a `FdEntry::Vfs`.
- Bridge sync→async: `embassy_futures::block_on` dentro le host fns.
- `user/init/src/main.rs`: aggiunto smoke `open /dev/null → write 10
  → drop close → println("vfs smoke ok")`.
- `Makefile` HELLO → `init.wasm: vfs smoke ok`.

## Perché

Terzo task dello Step 10. Materializza I/O wasm↔VFS via WASIX
host fns reali. Sblocca apps wasm che leggono/scrivono file.

## File toccati

- kernel/src/wasm/host/path.rs (nuovo)
- kernel/src/wasm/host/fd.rs
- kernel/src/wasm/host/mod.rs
- user/init/src/main.rs
- user-bin/init.wasm (rigenerato)
- Makefile
- CHANGELOG/63-26-05-28-wasix-vfs-host-fns.md (nuovo)
```

```bash
git add kernel/src/wasm/host/ user/init/src/main.rs user-bin/init.wasm Makefile CHANGELOG/63-26-05-28-wasix-vfs-host-fns.md
git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit -m "feat(rust): WASIX path_open + fd_read/seek/close on VFS

path_open resolves a wasm-side absolute path through ruos's tmpfs,
allocates a wasm-side FD slot, returns it. fd_read/fd_write/fd_seek/
fd_close then dispatch to the VFS Fd stored in the slot.

Bridge between wasmi's sync host fns and ruos's async VFS uses
embassy-futures::block_on from inside the wasm_task embassy task.

init.wasm now exercises the full path: open /dev/null, write 10
bytes, close, and reports 'init.wasm: vfs smoke ok'.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: clock + random + stdin (keyboard queue)

**Files:**
- Create: `kernel/src/wasm/host/clock.rs`
- Create: `kernel/src/wasm/host/random.rs`
- Modify: `kernel/src/wasm/state.rs` (add FdEntry::Stdin → keyboard queue)
- Modify: `kernel/src/wasm/host/fd.rs` (route FD 0 → keyboard queue in fd_read)
- Modify: `kernel/src/wasm/host/mod.rs` (link clock + random)
- Modify: `user/init/src/main.rs` (print uptime + 16 random bytes hex)
- Modify: `Makefile` (HELLO)

**Smoke contract:** sentinel `init.wasm: clock_rand ok`.

- [ ] **Step 4.1: HELLO bump**

```makefile
HELLO := init.wasm: clock_rand ok
```

- [ ] **Step 4.2: Verify fail**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 30 make run-test 2>&1 | tail -10'
```

- [ ] **Step 4.3: `kernel/src/wasm/host/clock.rs`**

```rust
//! WASIX clock host fns. Backed by ruos's TICKS atomic (100 Hz).

use wasmi::{Caller, Linker, errors::Error};
use crate::wasm::state::RuntimeState;
use crate::wasm::host::lifecycle::{wasm_memory, WasiTrap};

const TICK_NS: u64 = 10_000_000; // 100 Hz → 10 ms per tick → 10^7 ns

pub fn clock_time_get(
    caller: Caller<'_, RuntimeState>,
    _clock_id: i32,
    _precision: i64,
    time_ptr: i32,
) -> Result<i32, Error> {
    let ticks = crate::timer::ticks();
    let nanos: u64 = ticks * TICK_NS;
    let mem = wasm_memory(&caller)?;
    mem.write(&caller, time_ptr as usize, &nanos.to_le_bytes())
        .map_err(|_| Error::host(WasiTrap { exit_code: -1 }))?;
    Ok(0)
}

pub fn clock_res_get(
    caller: Caller<'_, RuntimeState>,
    _clock_id: i32,
    res_ptr: i32,
) -> Result<i32, Error> {
    let mem = wasm_memory(&caller)?;
    mem.write(&caller, res_ptr as usize, &TICK_NS.to_le_bytes())
        .map_err(|_| Error::host(WasiTrap { exit_code: -1 }))?;
    Ok(0)
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker
        .func_wrap("wasi_snapshot_preview1", "clock_time_get", clock_time_get)?
        .func_wrap("wasi_snapshot_preview1", "clock_res_get", clock_res_get)?;
    Ok(())
}
```

- [ ] **Step 4.4: `kernel/src/wasm/host/random.rs`**

```rust
//! WASIX random host fn. Weak xorshift PRNG seeded from TICKS at
//! kernel boot. Step 14 (SSH) replaces with RDRAND-backed CSPRNG.

use core::sync::atomic::{AtomicU64, Ordering};
use wasmi::{Caller, Linker, errors::Error};
use crate::wasm::state::RuntimeState;
use crate::wasm::host::lifecycle::{wasm_memory, WasiTrap};

static STATE: AtomicU64 = AtomicU64::new(0);

fn ensure_seeded() {
    if STATE.load(Ordering::Relaxed) == 0 {
        // Seed from TICKS XOR a constant so 0 ticks isn't a zero seed.
        let t = crate::timer::ticks();
        let seed = t.wrapping_mul(0x2545F4914F6CDD1D) ^ 0xDEADBEEFCAFEBABE;
        STATE.store(seed | 1, Ordering::Relaxed);
    }
}

fn next() -> u64 {
    ensure_seeded();
    let mut x = STATE.load(Ordering::Relaxed);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    STATE.store(x, Ordering::Relaxed);
    x
}

pub fn random_get(
    caller: Caller<'_, RuntimeState>,
    buf_ptr: i32,
    buf_len: i32,
) -> Result<i32, Error> {
    let mem = wasm_memory(&caller)?;
    let mut remaining = buf_len as usize;
    let mut offset = buf_ptr as usize;
    while remaining > 0 {
        let chunk = next().to_le_bytes();
        let n = remaining.min(8);
        mem.write(&caller, offset, &chunk[..n])
            .map_err(|_| Error::host(WasiTrap { exit_code: -1 }))?;
        offset += n;
        remaining -= n;
    }
    Ok(0)
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker.func_wrap("wasi_snapshot_preview1", "random_get", random_get)?;
    Ok(())
}
```

- [ ] **Step 4.5: Wire stdin (FD 0) to the keyboard queue**

Edit `kernel/src/wasm/state.rs`. Add a new variant to `FdEntry`:

```rust
pub enum FdEntry {
    Stdin,                  // reads from keyboard::queue::read_char()
    StdoutConsole,
    Vfs(crate::vfs::Fd),
}
```

And in `RuntimeState::new()`, set FD 0:

```rust
        fds[0] = Some(FdEntry::Stdin);
```

Then in `kernel/src/wasm/host/fd.rs`, extend `fd_read` to dispatch `Stdin`:

```rust
        match caller.data().fds.get(fd as usize).and_then(|x| x.as_ref()) {
            Some(FdEntry::Stdin) => {
                // Block on one char from the keyboard queue.
                let b = embassy_futures::block_on(crate::keyboard::queue::read_char());
                let mem2 = wasm_memory(&caller)?;
                mem2.write(&caller, buf_ptr as usize, &[b])
                    .map_err(|_| Error::host(WasiTrap { exit_code: -1 }))?;
                total += 1;
                write_u32(&mem, &caller, nread_ptr, total)?;
                return Ok(0);
            }
            Some(FdEntry::Vfs(vfd)) => { /* existing VFS read path */ }
            _ => return Ok(8),
        }
```

(Adapt for control flow — the existing `fd_read` from Step 3.4 needs a Stdin arm added.)

- [ ] **Step 4.6: Update `host/mod.rs`**

```rust
pub mod lifecycle;
pub mod fd;
pub mod path;
pub mod clock;
pub mod random;
// (sock module comes in Task 6)

use wasmi::{Linker, errors::Error};
use crate::wasm::state::RuntimeState;

pub fn install(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    lifecycle::link(linker)?;
    fd::link(linker)?;
    path::link(linker)?;
    clock::link(linker)?;
    random::link(linker)?;
    Ok(())
}
```

- [ ] **Step 4.7: Extend `user/init/src/main.rs`**

```rust
use std::fs::OpenOptions;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    println!("\x1b[1;32m╔══════════════════════════════════╗");
    println!("║         Welcome to ruos          ║");
    println!("║   wasm32-wasip1 / WASIX host     ║");
    println!("╚══════════════════════════════════╝\x1b[0m");

    // VFS smoke (Task 3).
    if let Ok(mut f) = OpenOptions::new().write(true).open("/dev/null") {
        if f.write_all(b"0123456789").is_ok() {
            println!("init.wasm: vfs smoke ok");
        }
    }

    // Clock + random smoke (Task 4).
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let ms = elapsed.as_millis();

    let mut rand_buf = [0u8; 16];
    getrandom::getrandom(&mut rand_buf).unwrap();

    print!("init.wasm: uptime_ms={} rand=", ms);
    for b in rand_buf { print!("{:02x}", b); }
    println!();

    println!("init.wasm: clock_rand ok");
}
```

Add `getrandom = "0.2"` to `user/init/Cargo.toml` `[dependencies]`:

```toml
[dependencies]
getrandom = "0.2"
```

`getrandom` 0.2 supports `wasm32-wasi`/`wasm32-wasip1` and calls `random_get` natively (no extra feature flag needed for wasi target).

- [ ] **Step 4.8: Rebuild + test**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make user-wasm && make build 2>&1 | tail -10 && timeout 30 make run-test 2>&1 | tail -20'
```

Expected serial tail:
```
... banner ...
init.wasm: vfs smoke ok
init.wasm: uptime_ms=2347 rand=a3f1...
init.wasm: clock_rand ok
ruos: init.wasm exited cleanly
```

- [ ] **Step 4.9: Changelog + commit**

`CHANGELOG/64-26-05-28-wasix-clock-random.md`:

```markdown
# 64 — clock + random + stdin host fns (Step 10 Task 4)

**Data:** 2026-05-28

## Cosa

- `kernel/src/wasm/host/clock.rs` (nuovo): `clock_time_get`,
  `clock_res_get`, backed da `timer::ticks() * 10ms`.
- `kernel/src/wasm/host/random.rs` (nuovo): `random_get`, xorshift
  weak prng seedato da TICKS al primo accesso. Step 14 sostituirà
  con RDRAND-backed CSPRNG.
- `RuntimeState`: FD 0 = `FdEntry::Stdin` → `keyboard::queue::read_char()`.
- `fd_read`: ramo Stdin che chiama keyboard queue via block_on.
- `user/init`: aggiunto print uptime_ms + 16 byte rand hex (via crate
  `getrandom = "0.2"` che mappa a `random_get` su wasm32-wasi*).
- HELLO → `init.wasm: clock_rand ok`.

## Perché

Quarto task dello Step 10. Sblocca apps wasm che leggono tempo
(es. timeouts, rate limiting) e usano RNG (es. session token,
TCP ISN, futuri SSH session keys quando arriverà RDRAND).

## File toccati

- kernel/src/wasm/host/clock.rs (nuovo)
- kernel/src/wasm/host/random.rs (nuovo)
- kernel/src/wasm/host/mod.rs
- kernel/src/wasm/host/fd.rs
- kernel/src/wasm/state.rs
- user/init/Cargo.toml
- user/init/src/main.rs
- user-bin/init.wasm (rigenerato)
- Makefile
- CHANGELOG/64-26-05-28-wasix-clock-random.md (nuovo)
```

```bash
git add kernel/src/wasm/ user/init/ user-bin/init.wasm Makefile CHANGELOG/64-26-05-28-wasix-clock-random.md
git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit -m "feat(rust): WASIX clock_time_get + random_get + stdin from kbd queue

clock_time_get reports TICKS * 10ms as nanoseconds; clock_res_get
matches. random_get uses a xorshift PRNG seeded from TICKS — weak
randomness, sufficient for non-crypto needs at this stage. Step 14
(SSH) will substitute RDRAND-backed CSPRNG.

FD 0 (stdin) reads from the async keyboard queue from Step 9 via
block_on; wasm apps can now read a single byte of user input.

init.wasm now also prints its uptime in ms and 16 random bytes
in hex, exercising both host fns end-to-end.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: smoltcp + Loopback + `net_poll_task`

**Files:**
- Create: `kernel/src/net/{mod,loopback,sockets}.rs`
- Modify: `kernel/Cargo.toml` (add smoltcp)
- Modify: `kernel/src/main.rs` (`mod net;` + `net::init()` in kmain)
- Modify: `kernel/src/executor/mod.rs` (spawn `net_poll_task`)
- Modify: `Makefile` (HELLO)

**Smoke contract:** sentinel `ruos: net init ok addr=127.0.0.1/8`.

- [ ] **Step 5.1: HELLO bump**

```makefile
HELLO := ruos: net init ok addr=127.0.0.1/8
```

- [ ] **Step 5.2: Verify fail**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 30 make run-test 2>&1 | tail -10'
```

- [ ] **Step 5.3: Add smoltcp dep**

`kernel/Cargo.toml`:

```toml
smoltcp = { version = "0.11", default-features = false, features = ["alloc", "medium-ethernet", "medium-ip", "proto-ipv4", "socket-tcp"] }
```

The exact features set may require iteration. The minimum we need:
- `alloc` (use heap)
- `proto-ipv4` (IPv4 stack)
- `socket-tcp` (TCP sockets)
- `medium-ip` (raw IP packets, what Loopback emits — NOT `medium-ethernet`, since loopback has no L2 header)

If 0.11 doesn't expose those exact names, fall back to 0.10 or check `cargo doc --package smoltcp --open`.

- [ ] **Step 5.4: `kernel/src/net/loopback.rs`**

```rust
//! Loopback device: smoltcp's built-in zero-config medium-ip
//! device. Wraps the type re-export for clarity.

pub use smoltcp::phy::Loopback;

pub fn new() -> Loopback {
    Loopback::new(smoltcp::phy::Medium::Ip)
}
```

- [ ] **Step 5.5: `kernel/src/net/mod.rs`**

```rust
//! Network stack — smoltcp on a Loopback device. NIC real driver
//! arrives at Step 14; today only 127.0.0.1/8 traffic.

pub mod loopback;
pub mod sockets;

use spin::Mutex;
use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::time::Instant;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr};
use crate::kprintln;

pub struct NetState {
    pub iface: Interface,
    pub device: loopback::Loopback,
    pub sockets: SocketSet<'static>,
}

pub static NET: Mutex<Option<NetState>> = Mutex::new(None);

pub fn init() {
    let mut device = loopback::new();
    let now = Instant::from_millis(crate::timer::ticks() as i64 * 10);

    let config = Config::new(HardwareAddress::Ip);
    let mut iface = Interface::new(config, &mut device, now);
    iface.update_ip_addrs(|addrs| {
        addrs.push(IpCidr::new(IpAddress::v4(127, 0, 0, 1), 8)).unwrap();
    });

    let sockets = SocketSet::new(alloc::vec::Vec::new());
    *NET.lock() = Some(NetState { iface, device, sockets });
    kprintln!("ruos: net init ok addr=127.0.0.1/8");
}

/// Called periodically by `net_poll_task` (every 10 ms).
pub fn poll() {
    use x86_64::instructions::interrupts::without_interrupts;
    without_interrupts(|| {
        let mut g = NET.lock();
        if let Some(net) = g.as_mut() {
            let now = Instant::from_millis(crate::timer::ticks() as i64 * 10);
            let _ = net.iface.poll(now, &mut net.device, &mut net.sockets);
        }
    });
}
```

- [ ] **Step 5.6: `kernel/src/net/sockets.rs`**

(Task 5 leaves this empty/stub; Task 6 fills it.)

```rust
//! Socket pool (kernel-side FD ↔ smoltcp SocketHandle). Populated
//! by sock_* host fns in Task 6.

// Empty for Task 5.
```

- [ ] **Step 5.7: Add `mod net;` in `main.rs`**

```rust
mod net;
```

And in `kmain`, call `net::init()` after `modules::mount_all()`:

```rust
    modules::mount_all();
    net::init();
```

- [ ] **Step 5.8: Spawn `net_poll_task` in executor**

In `kernel/src/executor/mod.rs`, after the existing wasm_task spawn, add the task definition (anywhere in the file):

```rust
#[embassy_executor::task]
async fn net_poll_task() {
    loop {
        crate::net::poll();
        delay::Delay::ticks(1).await; // 10 ms
    }
}
```

And in the `run()` closure, before spawning wasm_task:

```rust
    spawner.spawn(net_poll_task()).unwrap();
    spawner.spawn(wasm_task("/init.wasm")).unwrap();
```

- [ ] **Step 5.9: Build**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -20'
```

Expected: smoltcp compiles, links. May surface no_std issues (smoltcp depends on `managed`, `byteorder`); usually fine with `default-features = false`.

If smoltcp complains about missing `defmt` or `log` features, add `[features]` block to disable them or update the feature list:
```toml
smoltcp = { version = "0.11", default-features = false, features = ["alloc", "medium-ip", "proto-ipv4", "socket-tcp"] }
```

- [ ] **Step 5.10: Test**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 30 make run-test 2>&1 | tail -20'
```

Expected: `ruos: net init ok addr=127.0.0.1/8` in serial. async tick continues unaffected.

- [ ] **Step 5.11: Changelog + commit**

`CHANGELOG/65-26-05-28-wasix-smoltcp-loopback.md`:

```markdown
# 65 — smoltcp + Loopback device + net_poll_task (Step 10 Task 5)

**Data:** 2026-05-28

## Cosa

- Aggiunta dep `smoltcp = "0.11"` `default-features=false` con
  `alloc + medium-ip + proto-ipv4 + socket-tcp`.
- `kernel/src/net/mod.rs` (nuovo): `NetState` globale (Interface +
  Loopback device + SocketSet) in `Mutex<Option<_>>`. `init()` lo
  popola con IP 127.0.0.1/8. `poll()` chiamata periodica.
- `kernel/src/net/loopback.rs` (nuovo): wrapper triviale su
  `smoltcp::phy::Loopback`.
- `kernel/src/net/sockets.rs` (nuovo, vuoto): placeholder per Task 6.
- `kmain` chiama `net::init()` dopo `modules::mount_all()`.
- `executor` spawna `net_poll_task` accanto agli altri 3 task. 10ms
  cadenza via `Delay::ticks(1)`.
- HELLO → `ruos: net init ok addr=127.0.0.1/8`.

## Perché

Quinto task dello Step 10. Mette in piedi lo stack di rete embedded
sopra Loopback. Sblocca Task 6 (sock_* host fns su 127.0.0.1).
Niente NIC reale fino a Step 14.

## File toccati

- kernel/Cargo.toml
- kernel/src/net/{mod,loopback,sockets}.rs (nuovi)
- kernel/src/main.rs
- kernel/src/executor/mod.rs
- Makefile
- CHANGELOG/65-26-05-28-wasix-smoltcp-loopback.md (nuovo)
```

```bash
git add kernel/Cargo.toml kernel/Cargo.lock kernel/src/net/ kernel/src/main.rs kernel/src/executor/mod.rs Makefile CHANGELOG/65-26-05-28-wasix-smoltcp-loopback.md
git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit -m "feat(rust): smoltcp + Loopback device + net_poll_task

smoltcp 0.11 no_std + alloc + medium-ip + proto-ipv4 + socket-tcp.
Loopback device with 127.0.0.1/8. NetState in a global Mutex,
populated by net::init() called after modules::mount_all().
net_poll_task spawned in executor::run, drives iface.poll() every
10ms via Delay::ticks(1).

Sets up the stack for Task 6 to wire WASIX sock_* host fns onto
smoltcp socket handles. No real NIC until Step 14.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: `sock_*` host fns + server/client demo + final HELLO

**Files:**
- Modify: `kernel/src/net/sockets.rs` (SocketPool + async accept/connect/recv/send)
- Create: `kernel/src/wasm/host/sock.rs`
- Modify: `kernel/src/wasm/state.rs` (add FdEntry::Socket)
- Modify: `kernel/src/wasm/host/mod.rs` (link sock)
- Create: `user/server/{Cargo.toml,src/main.rs}` + `user/client/{Cargo.toml,src/main.rs}`
- Modify: `user/Cargo.toml` (members += server, client)
- Modify: `user-bin/server.wasm` and `user-bin/client.wasm` (build output)
- Modify: `limine.conf` (declare server/client modules)
- Modify: `Makefile` (build server + client wasm; HELLO)
- Modify: `kernel/src/executor/mod.rs` (spawn 3 wasm_tasks: init, server, client)

**Smoke contract:** sentinel `client.wasm: rx='pong'` (no `ruos:` prefix, comes from wasm stdout).

**Effort warning:** this is the longest task. Likely 1-3 build iterations to get smoltcp socket API + wasmi memory access patterns right.

- [ ] **Step 6.1: HELLO bump**

```makefile
HELLO := client.wasm: rx='pong'
```

- [ ] **Step 6.2: Build server + client placeholders**

```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && mkdir -p user/server/src user/client/src && printf "\x00" > user-bin/server.wasm && printf "\x00" > user-bin/client.wasm'
```

- [ ] **Step 6.3: `user/Cargo.toml` members**

```toml
[workspace]
resolver = "2"
members = ["init", "server", "client"]

[profile.release]
opt-level = "s"
lto = true
panic = "abort"
strip = true
```

- [ ] **Step 6.4: `user/server/{Cargo.toml,src/main.rs}`**

`user/server/Cargo.toml`:

```toml
[package]
name = "server"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "server"
path = "src/main.rs"
```

`user/server/src/main.rs`:

```rust
use std::io::{Read, Write};
use std::net::TcpListener;

fn main() {
    let listener = TcpListener::bind("127.0.0.1:8080").expect("bind");
    println!("server.wasm: listening on 127.0.0.1:8080");
    let (mut stream, _) = listener.accept().expect("accept");
    println!("server.wasm: accepted");
    let mut buf = [0u8; 32];
    let n = stream.read(&mut buf).expect("read");
    let received = core::str::from_utf8(&buf[..n]).unwrap_or("?");
    println!("server.wasm: rx='{}' tx='pong'", received);
    stream.write_all(b"pong").expect("write");
}
```

- [ ] **Step 6.5: `user/client/{Cargo.toml,src/main.rs}`**

`user/client/Cargo.toml`:

```toml
[package]
name = "client"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "client"
path = "src/main.rs"
```

`user/client/src/main.rs`:

```rust
use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

fn main() {
    // Give the server a moment to bind/listen. wasi-libc on wasm32-wasip1
    // doesn't fully support threads; sleep instead.
    thread::sleep(Duration::from_millis(200));

    let mut stream = TcpStream::connect("127.0.0.1:8080").expect("connect");
    stream.write_all(b"ping").expect("write");
    println!("client.wasm: tx='ping'");

    let mut buf = [0u8; 32];
    let n = stream.read(&mut buf).expect("read");
    let received = core::str::from_utf8(&buf[..n]).unwrap_or("?");
    println!("client.wasm: rx='{}'", received);
}
```

- [ ] **Step 6.6: Update Makefile to build all three .wasm**

```makefile
USER_WASMS := user-bin/init.wasm user-bin/server.wasm user-bin/client.wasm

user-bin/init.wasm: user/init/src/main.rs user/init/Cargo.toml user/Cargo.toml
	cd user && cargo build --target wasm32-wasip1 --release -p init
	cp user/target/wasm32-wasip1/release/init.wasm user-bin/init.wasm

user-bin/server.wasm: user/server/src/main.rs user/server/Cargo.toml user/Cargo.toml
	cd user && cargo build --target wasm32-wasip1 --release -p server
	cp user/target/wasm32-wasip1/release/server.wasm user-bin/server.wasm

user-bin/client.wasm: user/client/src/main.rs user/client/Cargo.toml user/Cargo.toml
	cd user && cargo build --target wasm32-wasip1 --release -p client
	cp user/target/wasm32-wasip1/release/client.wasm user-bin/client.wasm

.PHONY: user-wasm
user-wasm: $(USER_WASMS)

$(ISO): $(KERNEL_ELF) $(LIMINE_DEPS) limine.conf $(USER_WASMS)
```

Also in the iso staging step, add copies for the new modules:
```makefile
	cp user-bin/init.wasm build/iso_root/
	cp user-bin/server.wasm build/iso_root/
	cp user-bin/client.wasm build/iso_root/
```

- [ ] **Step 6.7: Update `limine.conf` to declare all 3 modules**

```
timeout: 0
default_entry: 1

/ruos
    protocol: limine
    kernel_path: boot():/boot/kernel.elf
    module_path: boot():/init.wasm
    module_cmdline: /init.wasm
    module_path: boot():/server.wasm
    module_cmdline: /server.wasm
    module_path: boot():/client.wasm
    module_cmdline: /client.wasm
```

- [ ] **Step 6.8: Build user-wasm to verify all three compile**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make user-wasm 2>&1 | tail -10 && ls -la user-bin/'
```

Expected three real `.wasm` files. If a build fails (TcpStream not supported), check: wasi-libc on wasm32-wasip1 has TCP support since 2024, should be fine. If not, may need to target `wasm32-wasi` (older) or use `wasi-net` crate.

- [ ] **Step 6.9: Implement `kernel/src/net/sockets.rs`**

```rust
//! Kernel-side socket pool. Each wasm-side FD that's a socket maps
//! to a `smoltcp::SocketHandle` here. The wasm host fns (`sock_*`)
//! manipulate this pool; the underlying smoltcp `Interface` is driven
//! by `net_poll_task` in `crate::net`.

use alloc::vec::Vec;
use spin::Mutex;
use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer, State};
use smoltcp::wire::{IpAddress, IpEndpoint};

const BUF_SIZE: usize = 4096;

pub struct SockPool {
    inner: Mutex<Vec<Option<SockEntry>>>,
}

pub struct SockEntry {
    pub handle: SocketHandle,
    pub kind: SockKind,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SockKind {
    Tcp,
}

pub static POOL: SockPool = SockPool { inner: Mutex::new(Vec::new()) };

impl SockPool {
    pub fn alloc_tcp(&self) -> usize {
        use x86_64::instructions::interrupts::without_interrupts;
        without_interrupts(|| {
            // smoltcp socket buffer
            let rx = SocketBuffer::new(alloc::vec![0u8; BUF_SIZE]);
            let tx = SocketBuffer::new(alloc::vec![0u8; BUF_SIZE]);
            let socket = TcpSocket::new(rx, tx);
            // Add to global SocketSet
            let mut g = crate::net::NET.lock();
            let net = g.as_mut().expect("net not initialized");
            let handle = net.sockets.add(socket);
            drop(g);
            let mut inner = self.inner.lock();
            let entry = SockEntry { handle, kind: SockKind::Tcp };
            for (i, slot) in inner.iter_mut().enumerate() {
                if slot.is_none() {
                    *slot = Some(entry);
                    return i;
                }
            }
            inner.push(Some(entry));
            inner.len() - 1
        })
    }
    pub fn handle(&self, idx: usize) -> Option<SocketHandle> {
        let g = self.inner.lock();
        g.get(idx).and_then(|x| x.as_ref()).map(|e| e.handle)
    }
}

/// Async bind+listen on the smoltcp socket. The smoltcp TCP socket
/// API binds during `listen()`.
pub fn listen(handle: SocketHandle, port: u16) -> Result<(), &'static str> {
    use x86_64::instructions::interrupts::without_interrupts;
    without_interrupts(|| {
        let mut g = crate::net::NET.lock();
        let net = g.as_mut().expect("net not initialized");
        let s = net.sockets.get_mut::<TcpSocket>(handle);
        s.listen(port).map_err(|_| "listen failed")
    })
}

pub async fn accept(handle: SocketHandle) -> Result<(), &'static str> {
    // Poll until the socket transitions to Established (smoltcp listen
    // sockets auto-accept incoming connections on the same handle).
    loop {
        let ready = check_state(handle, State::Established);
        if ready { return Ok(()); }
        crate::executor::delay::Delay::ticks(1).await;
    }
}

pub async fn connect(handle: SocketHandle, addr: IpEndpoint, local_port: u16) -> Result<(), &'static str> {
    use x86_64::instructions::interrupts::without_interrupts;
    {
        let _ = without_interrupts(|| {
            let mut g = crate::net::NET.lock();
            let net = g.as_mut().expect("net not initialized");
            let ctx = net.iface.context();
            let s = net.sockets.get_mut::<TcpSocket>(handle);
            s.connect(ctx, addr, IpEndpoint::new(IpAddress::v4(127,0,0,1), local_port))
                .map_err(|_| "connect failed")
        });
    }
    loop {
        let ready = check_state(handle, State::Established);
        if ready { return Ok(()); }
        crate::executor::delay::Delay::ticks(1).await;
    }
}

pub async fn recv(handle: SocketHandle, buf: &mut [u8]) -> Result<usize, &'static str> {
    use x86_64::instructions::interrupts::without_interrupts;
    loop {
        let n: Option<usize> = without_interrupts(|| {
            let mut g = crate::net::NET.lock();
            let net = g.as_mut().expect("net not initialized");
            let s = net.sockets.get_mut::<TcpSocket>(handle);
            if !s.can_recv() {
                return None;
            }
            let n = s.recv_slice(buf).ok()?;
            Some(n)
        });
        if let Some(n) = n {
            if n > 0 { return Ok(n); }
        }
        crate::executor::delay::Delay::ticks(1).await;
    }
}

pub async fn send(handle: SocketHandle, buf: &[u8]) -> Result<usize, &'static str> {
    use x86_64::instructions::interrupts::without_interrupts;
    loop {
        let n: Option<usize> = without_interrupts(|| {
            let mut g = crate::net::NET.lock();
            let net = g.as_mut().expect("net not initialized");
            let s = net.sockets.get_mut::<TcpSocket>(handle);
            if !s.can_send() {
                return None;
            }
            let n = s.send_slice(buf).ok()?;
            Some(n)
        });
        if let Some(n) = n {
            if n > 0 { return Ok(n); }
        }
        crate::executor::delay::Delay::ticks(1).await;
    }
}

fn check_state(handle: SocketHandle, target: State) -> bool {
    use x86_64::instructions::interrupts::without_interrupts;
    without_interrupts(|| {
        let g = crate::net::NET.lock();
        let net = g.as_ref().expect("net not initialized");
        net.sockets.get::<TcpSocket>(handle).state() == target
    })
}
```

This is a sketch. smoltcp 0.11 API specifics may differ (the `Interface::context()` method, the exact `connect` signature with remote+local endpoint). Adapt during implementation.

- [ ] **Step 6.10: `kernel/src/wasm/host/sock.rs`**

```rust
//! WASIX sock_* host fns. Bridges wasm-side socket FDs to
//! `crate::net::sockets::POOL` and underlying smoltcp.

use wasmi::{Caller, Linker, errors::Error};
use crate::wasm::state::{RuntimeState, FdEntry};
use crate::wasm::host::lifecycle::{wasm_memory, write_u32, read_u32, WasiTrap};
use crate::net::sockets::POOL;
use smoltcp::wire::{IpAddress, IpEndpoint};

/// sock_open(_af, _type, _proto, sock_fd_ptr) -> errno
/// We ignore af/type/proto for Step 10 (TCP only).
pub fn sock_open(
    mut caller: Caller<'_, RuntimeState>,
    _af: i32,
    _ty: i32,
    _proto: i32,
    sock_fd_ptr: i32,
) -> Result<i32, Error> {
    let idx = POOL.alloc_tcp();
    let state = caller.data_mut();
    let mut wfd: Option<usize> = None;
    for (i, slot) in state.fds.iter_mut().enumerate().skip(3) {
        if slot.is_none() {
            *slot = Some(FdEntry::Socket(idx));
            wfd = Some(i);
            break;
        }
    }
    let wfd = wfd.unwrap_or_else(|| {
        state.fds.push(Some(FdEntry::Socket(idx)));
        state.fds.len() - 1
    });
    let mem = wasm_memory(&caller)?;
    write_u32(&mem, &caller, sock_fd_ptr, wfd as u32)?;
    Ok(0)
}

/// sock_bind: wasi/wasix abstract; we just record the port for now,
/// actually bind on listen.
pub fn sock_bind(_: Caller<'_, RuntimeState>, _: i32, _: i32, _: i32, _: i32) -> Result<i32, Error> {
    Ok(0) // success no-op
}

/// sock_listen(fd, port, _backlog) -> errno
pub fn sock_listen(
    caller: Caller<'_, RuntimeState>,
    fd: i32,
    port: i32,
    _backlog: i32,
) -> Result<i32, Error> {
    let idx = match caller.data().fds.get(fd as usize).and_then(|x| x.as_ref()) {
        Some(FdEntry::Socket(i)) => *i,
        _ => return Ok(8), // EBADF
    };
    let handle = POOL.handle(idx).ok_or_else(|| Error::host(WasiTrap { exit_code: -1 }))?;
    crate::net::sockets::listen(handle, port as u16)
        .map_err(|_| Error::host(WasiTrap { exit_code: -1 }))?;
    Ok(0)
}

/// sock_accept(fd, new_fd_ptr) -> errno
///
/// smoltcp's TCP listen socket transitions to Established when a
/// peer connects; we don't create a new socket — we re-use the same
/// FD as the accepted connection.
pub fn sock_accept(
    caller: Caller<'_, RuntimeState>,
    fd: i32,
    new_fd_ptr: i32,
) -> Result<i32, Error> {
    let idx = match caller.data().fds.get(fd as usize).and_then(|x| x.as_ref()) {
        Some(FdEntry::Socket(i)) => *i,
        _ => return Ok(8),
    };
    let handle = POOL.handle(idx).ok_or_else(|| Error::host(WasiTrap { exit_code: -1 }))?;
    embassy_futures::block_on(crate::net::sockets::accept(handle))
        .map_err(|_| Error::host(WasiTrap { exit_code: -1 }))?;
    let mem = wasm_memory(&caller)?;
    write_u32(&mem, &caller, new_fd_ptr, fd as u32)?;
    Ok(0)
}

/// sock_connect(fd, ip_ptr_ignored, port) -> errno
/// Simplified: ignore ip arg (assume 127.0.0.1); use port.
pub fn sock_connect(
    caller: Caller<'_, RuntimeState>,
    fd: i32,
    _ip_ptr: i32,
    port: i32,
) -> Result<i32, Error> {
    let idx = match caller.data().fds.get(fd as usize).and_then(|x| x.as_ref()) {
        Some(FdEntry::Socket(i)) => *i,
        _ => return Ok(8),
    };
    let handle = POOL.handle(idx).ok_or_else(|| Error::host(WasiTrap { exit_code: -1 }))?;
    let remote = IpEndpoint::new(IpAddress::v4(127,0,0,1), port as u16);
    // Local port: ephemeral. Pick a fixed one for Step 10 demo.
    embassy_futures::block_on(crate::net::sockets::connect(handle, remote, 49152))
        .map_err(|_| Error::host(WasiTrap { exit_code: -1 }))?;
    Ok(0)
}

/// Direct sock_recv / sock_send. For Step 10 we route them through
/// the regular fd_read/fd_write paths instead; this avoids duplicating
/// WASIX recv/send ABI. The standard wasi libc maps a socket FD's
/// read/write to fd_read/fd_write.

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker
        .func_wrap("wasi_snapshot_preview1", "sock_open", sock_open)?
        .func_wrap("wasi_snapshot_preview1", "sock_bind", sock_bind)?
        .func_wrap("wasi_snapshot_preview1", "sock_listen", sock_listen)?
        .func_wrap("wasi_snapshot_preview1", "sock_accept", sock_accept)?
        .func_wrap("wasi_snapshot_preview1", "sock_connect", sock_connect)?;
    Ok(())
}
```

Note: wasi-libc on `wasm32-wasip1` may NOT call WASIX-namespaced `sock_*` directly. It uses POSIX socket API mapped through different shims. Need to check by inspecting the import names of `server.wasm` after compile:

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && wasm-objdump -x user-bin/server.wasm 2>&1 | grep import | head -20'
```

(Install `wasm-objdump` via `wabt` package if missing.)

If `wasm32-wasip1` libc uses `sock_recv`/`sock_send` etc. (the WASI P1 ABI), implement those. If it uses different names (e.g., the WASI experimental sockets proposal), adapt accordingly.

- [ ] **Step 6.11: Extend `FdEntry`**

`kernel/src/wasm/state.rs`:

```rust
pub enum FdEntry {
    Stdin,
    StdoutConsole,
    Vfs(crate::vfs::Fd),
    Socket(usize), // index into net::sockets::POOL
}
```

Update `fd_write` and `fd_read` in `kernel/src/wasm/host/fd.rs` to dispatch `Socket(idx)`:

```rust
            Some(FdEntry::Socket(idx)) => {
                let handle = crate::net::sockets::POOL.handle(*idx)
                    .ok_or_else(|| Error::host(WasiTrap { exit_code: -1 }))?;
                let n = embassy_futures::block_on(crate::net::sockets::send(handle, &buf[..n]))
                    .map_err(|_| Error::host(WasiTrap { exit_code: -1 }))?;
                total += n as u32;
            }
```

Similar for fd_read (call `recv`).

- [ ] **Step 6.12: `host/mod.rs`**

```rust
pub mod lifecycle;
pub mod fd;
pub mod path;
pub mod clock;
pub mod random;
pub mod sock;

pub fn install(linker: &mut wasmi::Linker<RuntimeState>) -> Result<(), wasmi::errors::Error> {
    lifecycle::link(linker)?;
    fd::link(linker)?;
    path::link(linker)?;
    clock::link(linker)?;
    random::link(linker)?;
    sock::link(linker)?;
    Ok(())
}
```

- [ ] **Step 6.13: Spawn three wasm_tasks**

`kernel/src/executor/mod.rs`. Update the spawn closure:

```rust
    exec.run(|spawner| {
        spawner.spawn(tick_task()).unwrap();
        spawner.spawn(kbd_echo_task()).unwrap();
        spawner.spawn(net_poll_task()).unwrap();
        spawner.spawn(wasm_task("/init.wasm")).unwrap();
        spawner.spawn(wasm_task("/server.wasm")).unwrap();
        spawner.spawn(wasm_task("/client.wasm")).unwrap();
    })
```

Each `wasm_task` uses a separate slot from `pool_size = 3` — sufficient for these three.

- [ ] **Step 6.14: Rebuild + test**

```bash
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make user-wasm && make build 2>&1 | tail -15 && timeout 60 make run-test 2>&1 | tail -40'
```

Expected serial tail (intermixed with `async tick=N`):
```
ruos: mounted 3 boot modules
ruos: net init ok addr=127.0.0.1/8
ruos: executor up
ruos: async tick=0
... (banner)
init.wasm: vfs smoke ok
init.wasm: uptime_ms=... rand=...
init.wasm: clock_rand ok
ruos: init.wasm exited cleanly
server.wasm: listening on 127.0.0.1:8080
client.wasm: tx='ping'
server.wasm: accepted
server.wasm: rx='ping' tx='pong'
client.wasm: rx='pong'
ruos: client.wasm exited cleanly
ruos: server.wasm exited cleanly
```

If you see one of:
- `server.wasm: bind error: ...`: check sock_bind / sock_listen impls.
- `client.wasm: connect: ...`: client started before server bound. Increase the client's `thread::sleep(...)` to 500ms.
- Both `.wasm` hang silently: net_poll_task isn't running, or the recv/send loops never see data — verify with kprintln tracing.

- [ ] **Step 6.15: Changelog + commit**

`CHANGELOG/66-26-05-28-wasix-sockets-demo.md`:

```markdown
# 66 — sock_* host fns + server/client demo (Step 10 Task 6)

**Data:** 2026-05-28

## Cosa

- `kernel/src/net/sockets.rs`: `SockPool` (kernel-side socket
  table) + async `accept`/`connect`/`recv`/`send` poll-loop wrappers
  che cooperano col `net_poll_task`.
- `kernel/src/wasm/host/sock.rs` (nuovo): `sock_open`/`bind`/`listen`/
  `accept`/`connect`. Recv/send passano per `fd_read`/`fd_write` via
  nuova variante `FdEntry::Socket`.
- `kernel/src/wasm/state.rs`: `FdEntry::Socket(usize)`.
- `kernel/src/wasm/host/fd.rs`: dispatch socket per fd_read/fd_write.
- `user/{server,client}/`: due crate nuovi, build wasm32-wasip1.
- `limine.conf` + `Makefile` + `user/Cargo.toml`: 3 moduli totali
  (init + server + client).
- `executor::run` spawna tutti e 3 i wasm task.
- HELLO → `client.wasm: rx='pong'`.

## Perché

Sesto e ultimo task dello Step 10. Materializza il bootstrap WASIX
completo: tre binari wasm reali coordinano via TCP loopback in
ruos. Sblocca Step 13+ (bash.wasm, curl.wasm, ecc.) appena
matureremo l'ABI.

## File toccati

- kernel/src/net/sockets.rs
- kernel/src/wasm/host/sock.rs (nuovo)
- kernel/src/wasm/host/fd.rs
- kernel/src/wasm/host/mod.rs
- kernel/src/wasm/state.rs
- kernel/src/executor/mod.rs
- user/Cargo.toml
- user/{server,client}/{Cargo.toml,src/main.rs} (nuovi)
- user-bin/{server,client}.wasm (nuovi)
- limine.conf
- Makefile
- CHANGELOG/66-26-05-28-wasix-sockets-demo.md (nuovo)
```

```bash
git add kernel/src/net/sockets.rs kernel/src/wasm/ kernel/src/executor/mod.rs user/ user-bin/ limine.conf Makefile CHANGELOG/66-26-05-28-wasix-sockets-demo.md
git -c user.name="g.desolda" -c user.email="g.desolda@gmail.com" commit -m "feat(rust): WASIX sock_* host fns + TCP loopback server/client demo

SockPool maps wasm-side FD indices to smoltcp SocketHandle entries
in the global SocketSet. sock_open allocates a TCP socket;
sock_listen/connect kick off the smoltcp transitions; sock_accept,
sock_recv (via fd_read), sock_send (via fd_write) drive their
async wait loops cooperatively, yielding to net_poll_task between
checks.

server.wasm binds 127.0.0.1:8080, accepts one connection, reads
'ping', writes 'pong'. client.wasm sleeps 200ms, connects to the
server, writes 'ping', reads 'pong'. All three wasm tasks run as
embassy tasks alongside tick_task / kbd_echo_task / net_poll_task.

Step 10 milestone complete: wasmi runtime + ~25 WASIX host fns +
smoltcp loopback TCP. End-to-end demo: a real Rust binary
compiled to wasm32-wasip1 talks TCP to another real Rust binary,
both inside ruos.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Self-review (controller)

**Spec coverage:**
| Spec requirement | Implemented by |
|---|---|
| wasmi 0.36 default-features=false | Task 2 |
| Limine modules → tmpfs mount | Task 1 |
| `wasm_task` embassy task wrapper | Task 2 |
| `RuntimeState` per-instance | Task 2 |
| Host fns lifecycle (args/env/proc_exit) | Task 2 |
| Host fn fd_write to console | Task 2 |
| Host fn fd_read/seek/close on VFS | Task 3 |
| Host fn path_open + path_* | Task 3 |
| Host fn clock_*/random_* | Task 4 |
| Host fn fd_read from keyboard queue | Task 4 |
| smoltcp + Loopback + net_poll_task | Task 5 |
| sock_* host fns + SockPool | Task 6 |
| Demo: init/server/client.wasm | Tasks 2/3/4/6 |
| Final HELLO: client.wasm rx='pong' | Task 6 |

**Open risks (mentioned in spec):**
- wasmi 0.36 vs 0.32 API fallback: noted in Task 2 Step 2.4.
- `wasm32-wasip1` socket support: tested in Task 6 Step 6.8; if libc maps to non-WASIX import names, adapt sock.rs.
- smoltcp `Loopback` device feature flags: noted in Task 5 Step 5.9.

**Type consistency:** `RuntimeState`, `FdEntry`, `Runtime`, `wasm_task`, `net::poll`, `net::NET`, `net::sockets::POOL` all defined once, referenced consistently.

---

## After all tasks complete

1. `make build` clean.
2. `make run-test` PASS (sentinel `client.wasm: rx='pong'`).
3. Dispatch a final whole-implementation reviewer (superpowers:code-reviewer agent), audit ISR safety, lock disciplines (NET, POOL, RuntimeState, vfs locks), wasmi error propagation, smoltcp poll integration with embassy executor.
4. Non-blocking findings → `docs/followups/step-10.md`.
5. Manual smoke in VirtualBox: verify Welcome banner appears on framebuffer console, async tick continues, keyboard still echoes.
6. Merge `feature/wasix-bootstrap` → `main` no-ff, push origin/main, delete local branch.
