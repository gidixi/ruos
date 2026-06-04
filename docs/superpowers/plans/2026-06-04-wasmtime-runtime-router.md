# Wasmtime Runtime Router + WASI Linker + Build Pipeline (Plan #7)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Turn the proven Wasmtime AOT spike into a real runtime: run precompiled
`.cwasm` tools loaded from the VFS, linked against a no_std WASI Preview 1
implementation, dispatched by a shell/exec router (`*.cwasm` → Wasmtime, `*.wasm`
→ wasmi), with a reproducible Makefile build pipeline.

**Architecture:** A new `WtState` (per-instance fds/args/env/cwd) is the Wasmtime
`Store` data. WASI Preview 1 functions are hand-implemented on a
`wasmtime::Linker<WtState>` via `func_wrap`, reusing the SAME kernel services the
wasmi host fns already use (`crate::vfs`, `crate::pty`, `crate::proc`). Guest
memory is touched through one bounds-checked accessor (mirroring
`wasm/host/mem.rs`). The host build precompiles each `.cwasm` with
`tools/wt-precompile` using the exact settings the runtime expects (spike recipe).

**Foundation for:** Plan #4 (`ruos_gfx`) and Plan #6 (`ruos_proc`) extend the
same `Linker<WtState>`. The GUI app (`gui.cwasm`, built PC-side) needs WASI std,
so this plan unblocks it.

**Depends on:** the spike (`kernel/src/wasm/wt/`), already landed and GO.

**Build/run via WSL:** `wsl -d Ubuntu-22.04 -u root -e bash -lc 'cd /mnt/w/Work/GitHub/ruos && <cmd>'`
(see memory `ruos-build-env`). Verify with boot-checks self-tests + `make iso
CARGO_FEATURES=boot-checks` then QEMU `-cpu max` (NOT bare `make test-boot`
unless its smoke/init env is satisfied).

---

## File Structure

- Create: `kernel/src/wasm/wt/state.rs` — `WtState` (fds, args, env, cwd, exit).
- Create: `kernel/src/wasm/wt/mem.rs` — bounds-checked guest-memory accessor.
- Create: `kernel/src/wasm/wt/wasi.rs` — WASI Preview 1 functions on the Linker.
- Modify: `kernel/src/wasm/wt/mod.rs` — `run_cwasm(path)` entry; engine reuse.
- Modify: `kernel/src/wasm/exec_queue.rs` (or shell exec path) — router by extension.
- Modify: `Makefile` — `.cwasm` precompile rule + stage as Limine module.
- Modify: `limine.conf` — list `.cwasm` modules (test build).

---

## Task 1: `WtState` + engine reuse

**Files:** Create `kernel/src/wasm/wt/state.rs`; modify `kernel/src/wasm/wt/mod.rs`.

- [ ] **Step 1: Define `WtState`**

```rust
//! Per-instance state for a Wasmtime guest: open fds, argv/env, cwd, exit code.
//! Mirrors the wasmi `RuntimeState` but for the Wasmtime store.
use alloc::{vec::Vec, string::String};

pub enum WtFd {
    /// Standard streams + PTYs route through the PTY layer (pts index).
    Pty(usize),
    /// An open VFS file descriptor.
    Vfs(crate::vfs::Fd),
    Closed,
}

pub struct WtState {
    pub fds: Vec<WtFd>,
    pub args: Vec<Vec<u8>>,
    pub env: Vec<Vec<u8>>,
    pub cwd: String,
    pub exit: Option<i32>,
    /// pts index this guest's stdio is bound to.
    pub pts: usize,
}

impl WtState {
    pub fn new(pts: usize, args: Vec<Vec<u8>>) -> Self {
        // fd 0/1/2 → the bound PTY.
        let fds = alloc::vec![WtFd::Pty(pts), WtFd::Pty(pts), WtFd::Pty(pts)];
        Self { fds, args, env: Vec::new(), cwd: String::from("/"), exit: None, pts }
    }
}
```

- [ ] **Step 2: Cache one `Engine` (config is fixed)**

In `kernel/src/wasm/wt/mod.rs`, replace per-call `Engine::new` with a lazily
built shared engine (config never changes):

```rust
use spin::Once;
static ENGINE: Once<wasmtime::Engine> = Once::new();

pub fn engine() -> &'static wasmtime::Engine {
    ENGINE.call_once(|| wasmtime::Engine::new(&engine_config()).expect("wt engine"))
}
```

Keep `engine_config()` from the spike (x86 float ABI + fixed detect_host_feature).

- [ ] **Step 3: Build & boot-check (no behaviour change)**

Run: `make iso CARGO_FEATURES=boot-checks` + QEMU `-cpu max`; expect the existing
`wasmtime AOT hello ok` line still appears (run_hello can be switched to use
`engine()`).

- [ ] **Step 4: Commit**

```bash
git add kernel/src/wasm/wt/state.rs kernel/src/wasm/wt/mod.rs
git commit -m "feat(wt): WtState + shared Engine"
```

---

## Task 2: Bounds-checked guest memory accessor

**Files:** Create `kernel/src/wasm/wt/mem.rs`.

- [ ] **Step 1: Implement the accessor**

```rust
//! The single audited path to a Wasmtime guest's linear memory (mirrors the
//! wasmi `wasm/host/mem.rs` rule: no raw guest reads/writes elsewhere).
use wasmtime::{Caller, Extern, Memory};
use crate::wasm::wt::state::WtState;

fn memory(caller: &mut Caller<'_, WtState>) -> Option<Memory> {
    match caller.get_export("memory") {
        Some(Extern::Memory(m)) => Some(m),
        _ => None,
    }
}

/// Copy `buf` into guest memory at `ptr`. Returns false if out of bounds.
pub fn write(caller: &mut Caller<'_, WtState>, ptr: u32, buf: &[u8]) -> bool {
    if let Some(mem) = memory(caller) {
        mem.write(caller, ptr as usize, buf).is_ok()
    } else {
        false
    }
}

/// Read `len` bytes from guest memory at `ptr` into a Vec. None if OOB.
pub fn read(caller: &mut Caller<'_, WtState>, ptr: u32, len: u32) -> Option<alloc::vec::Vec<u8>> {
    let mem = memory(caller)?;
    let mut out = alloc::vec![0u8; len as usize];
    mem.read(caller, ptr as usize, &mut out).ok()?;
    Some(out)
}

/// Write a little-endian u32 to guest memory. False if OOB.
pub fn write_u32(caller: &mut Caller<'_, WtState>, ptr: u32, val: u32) -> bool {
    write(caller, ptr, &val.to_le_bytes())
}
```

`Memory::read/write` already bounds-check against the current memory size, so this
is the audited boundary.

- [ ] **Step 2: Build; Step 3: Commit**

```bash
git add kernel/src/wasm/wt/mem.rs
git commit -m "feat(wt): bounds-checked guest memory accessor"
```

---

## Task 3: WASI Preview 1 — core subset

WASI errno: 0 = success. Functions return i32 errno. Implement the minimal set a
`std` binary needs to start and do console + file I/O, reusing kernel services.

**Files:** Create `kernel/src/wasm/wt/wasi.rs`; modify `kernel/src/wasm/wt/mod.rs`.

- [ ] **Step 1: Implement representative functions (the pattern)**

```rust
//! WASI Preview 1 on a wasmtime::Linker<WtState>. Reuses crate::vfs / crate::pty
//! (the same services the wasmi host fns use), so semantics match across runtimes.
use wasmtime::Linker;
use crate::wasm::wt::state::{WtState, WtFd};
use crate::wasm::wt::mem;

const ERRNO_OK: i32 = 0;
const ERRNO_BADF: i32 = 8;
const ERRNO_INVAL: i32 = 28;

pub fn add_to_linker(linker: &mut Linker<WtState>) -> wasmtime::Result<()> {
    // proc_exit(code) -> !
    linker.func_wrap("wasi_snapshot_preview1", "proc_exit",
        |mut caller: wasmtime::Caller<'_, WtState>, code: i32| {
            caller.data_mut().exit = Some(code);
            // Trap to unwind out of guest execution promptly.
            Err::<(), _>(wasmtime::Error::msg("proc_exit"))
        })?;

    // fd_write(fd, iovs, iovs_len, nwritten) -> errno
    linker.func_wrap("wasi_snapshot_preview1", "fd_write",
        |mut caller: wasmtime::Caller<'_, WtState>, fd: i32, iovs: i32, iovs_len: i32, nwritten: i32| -> i32 {
            // Read the iovec array: pairs of (ptr:u32, len:u32).
            let table = match mem::read(&mut caller, iovs as u32, (iovs_len as u32) * 8) {
                Some(t) => t, None => return ERRNO_INVAL,
            };
            let mut total: u32 = 0;
            for i in 0..iovs_len as usize {
                let base = i * 8;
                let ptr = u32::from_le_bytes(table[base..base+4].try_into().unwrap());
                let len = u32::from_le_bytes(table[base+4..base+8].try_into().unwrap());
                let bytes = match mem::read(&mut caller, ptr, len) { Some(b) => b, None => return ERRNO_INVAL };
                match caller.data().fds.get(fd as usize) {
                    Some(WtFd::Pty(pts)) => { for b in &bytes { crate::pty::slave_output_push(*pts, *b); } }
                    Some(WtFd::Vfs(_)) => { /* TODO Task 4: VFS write */ }
                    _ => return ERRNO_BADF,
                }
                total += len;
            }
            if !mem::write_u32(&mut caller, nwritten as u32, total) { return ERRNO_INVAL; }
            ERRNO_OK
        })?;

    // args_sizes_get(argc_out, argv_buf_size_out) -> errno
    linker.func_wrap("wasi_snapshot_preview1", "args_sizes_get",
        |mut caller: wasmtime::Caller<'_, WtState>, argc: i32, buf_size: i32| -> i32 {
            let n = caller.data().args.len() as u32;
            let sz: u32 = caller.data().args.iter().map(|a| a.len() as u32 + 1).sum();
            if !mem::write_u32(&mut caller, argc as u32, n) { return ERRNO_INVAL; }
            if !mem::write_u32(&mut caller, buf_size as u32, sz) { return ERRNO_INVAL; }
            ERRNO_OK
        })?;

    // clock_time_get(id, precision, time_out) -> errno
    linker.func_wrap("wasi_snapshot_preview1", "clock_time_get",
        |mut caller: wasmtime::Caller<'_, WtState>, _id: i32, _prec: i64, out: i32| -> i32 {
            let ns = crate::rtc::now_unix_nanos(); // reuse existing clock source
            if !mem::write(&mut caller, out as u32, &ns.to_le_bytes()) { return ERRNO_INVAL; }
            ERRNO_OK
        })?;

    Ok(())
}
```

(Verify `crate::pty::slave_output_push` / the correct PTY write fn name with
`grep -n "pub fn .*push" kernel/src/pty/*.rs`; reuse whatever the wasmi `fd_write`
uses — see `kernel/src/wasm/host/fd.rs`.)

- [ ] **Step 2: Build & commit**

```bash
git add kernel/src/wasm/wt/wasi.rs kernel/src/wasm/wt/mod.rs
git commit -m "feat(wt): WASI p1 core subset (proc_exit, fd_write, args, clock)"
```

---

## Task 4: WASI Preview 1 — remaining functions

Implement the rest, each following Task 3's pattern, reusing the corresponding
logic from `kernel/src/wasm/host/{fd,path,clock,random}.rs`:

- [ ] `args_get`, `environ_sizes_get`, `environ_get`
- [ ] `fd_read` (PTY read = cooperative; see Task 6 for blocking), `fd_seek`,
      `fd_close`, `fd_fdstat_get`, `fd_prestat_get`, `fd_prestat_dir_name`
- [ ] `fd_readdir`
- [ ] `path_open`, `path_filestat_get`, `path_create_directory`,
      `path_unlink_file`, `path_remove_directory`, `path_rename`
- [ ] `random_get` (→ `crate::rng`), `clock_res_get`, `poll_oneoff` (stub/yield),
      `sched_yield`

For each: add a `linker.func_wrap(...)` with the real signature (cross-check the
wasmi impl in `host/*.rs`), copy its kernel-service call, build, and commit in
small groups. **No placeholder bodies** — port the actual logic.

- [ ] **Final step: Commit each group**

```bash
git commit -m "feat(wt): WASI p1 <group>"
```

---

## Task 5: `run_cwasm` entry + WtState wiring

**Files:** Modify `kernel/src/wasm/wt/mod.rs`.

- [ ] **Step 1: Implement `run_cwasm`**

```rust
/// Load and run a precompiled `.cwasm` on the given pts with argv. Returns the
/// guest exit code (0 if it returned without calling proc_exit).
pub fn run_cwasm(cwasm: &[u8], pts: usize, args: alloc::vec::Vec<alloc::vec::Vec<u8>>) -> i32 {
    use wasmtime::{Module, Store, Linker};
    let engine = engine();
    let module = match unsafe { Module::deserialize(engine, cwasm) } {
        Ok(m) => m, Err(e) => { crate::kprintln!("wt: deser {:?}", e); return 126; }
    };
    let mut store = Store::new(engine, WtState::new(pts, args));
    let mut linker = Linker::new(engine);
    if crate::wasm::wt::wasi::add_to_linker(&mut linker).is_err() { return 126; }
    // Plan #4/#6 also call: gfx::add_to_linker(&mut linker); proc_abi::add_to_linker(&mut linker);
    let instance = match linker.instantiate(&mut store, &module) {
        Ok(i) => i, Err(e) => { crate::kprintln!("wt: inst {:?}", e); return 126; }
    };
    // WASI command modules export `_start`.
    if let Ok(start) = instance.get_typed_func::<(), ()>(&mut store, "_start") {
        let _ = start.call(&mut store, ());
    }
    store.data().exit.unwrap_or(0)
}
```

- [ ] **Step 2: Build & commit**

```bash
git commit -am "feat(wt): run_cwasm entry with WASI linker"
```

---

## Task 6: Cooperative blocking (epoch yield)

`fd_read` on an empty PTY must not busy-wait. Use Wasmtime epoch interruption so a
would-block read yields to the executor and resumes on the next tick.

- [ ] **Step 1: Enable epoch interruption**

In `engine_config()` add `config.epoch_interruption(true);` (both precompiler and
runtime — re-run `tools/wt-precompile` and re-embed any demo cwasm afterward, the
settings hash changes).

- [ ] **Step 2: Drive epochs from the timer**

In the LAPIC timer tick path, call `engine().increment_epoch()` once per tick so
yielded guests get re-polled. (Add a small hook in `timer.rs` / the executor.)

- [ ] **Step 3: Make `fd_read` yield on empty PTY**

In the `fd_read` impl, if the PTY has no input, set an epoch deadline and return a
yield (or loop with `store.set_epoch_deadline`); resume reads the buffered bytes.
Mirror how the wasmi fiber suspends (`wasm/suspend.rs`) but via epoch.

- [ ] **Step 4: Build, run an interactive `.cwasm`, commit**

```bash
git commit -am "feat(wt): cooperative epoch-yield blocking for fd_read"
```

---

## Task 7: Runtime router

**Files:** Modify the shell/exec path (`kernel/src/wasm/exec_queue.rs` and/or
`user/shell`).

- [ ] **Step 1: Dispatch by extension**

Where a command path is resolved to `/bin/<cmd>.wasm`, also accept
`/bin/<cmd>.cwasm`: if the resolved file ends in `.cwasm`, read its bytes and call
`crate::wasm::wt::run_cwasm(&bytes, pts, args)`; otherwise the existing wasmi
`Fiber` path. Resolve `.cwasm` first, then `.wasm`.

```rust
// pseudo, in the resolver:
if let Some(bytes) = try_read(format!("/bin/{cmd}.cwasm")).await {
    return crate::wasm::wt::run_cwasm(&bytes, pts, args);
}
// else existing wasmi path for {cmd}.wasm
```

- [ ] **Step 2: Build; run a `.cwasm` tool from the shell; commit**

---

## Task 8: Makefile `.cwasm` pipeline

**Files:** Modify `Makefile`, `limine.conf`.

- [ ] **Step 1: Add precompile rules**

```make
WT_PRECOMPILE := tools/wt-precompile/target/release/wt-precompile

$(WT_PRECOMPILE): tools/wt-precompile/src/main.rs tools/wt-precompile/Cargo.toml
	source $$HOME/.cargo/env && cd tools/wt-precompile && cargo build --release

# Pattern: a wasm32-wasip1 tool -> portable .cwasm for the kernel runtime.
build/%.cwasm: user-bin/%.wasm $(WT_PRECOMPILE)
	@mkdir -p build
	$(WT_PRECOMPILE) $< $@
```

- [ ] **Step 2: Stage selected `.cwasm` as Limine modules**

Add the chosen `.cwasm` (e.g. `gui.cwasm`) to the ISO `for` loop and `limine.conf`
next to the `.wasm` tools, so they mount under `/bin`.

- [ ] **Step 3: Build the ISO, confirm the module mounts, commit**

---

## Task 9: Changelog

- [ ] Create `CHANGELOG/NN-26-06-04-wasmtime-runtime-router.md` (next free NN) and
  commit.

---

## Self-Review notes

- **Spec coverage:** implements spec §3 (router, build pipeline), §5 WASI side on
  Wasmtime, §7 blocking (epoch). Foundation for §4/§5.1.
- **Scope honesty:** Task 4 lists ~20 WASI fns to port one-for-one from the wasmi
  impls; each is mechanical but must carry REAL logic (no stubs except
  `poll_oneoff` which may yield). Consider splitting Task 4 into its own plan if
  it grows large.
- **Settings hash:** enabling `epoch_interruption` (Task 6) changes the AOT
  compatibility hash — the precompiler MUST set the same flag and all `.cwasm`
  must be regenerated. Pin both.
- **Verify names:** `crate::rtc::now_unix_nanos`, the PTY push/read fns, and
  `crate::vfs::Fd` are referenced from memory of the codebase — confirm exact
  names with grep before relying on them (Task 1 Step 1 / Task 3 Step 1).
