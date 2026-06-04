# ruos_proc ABI — Terminal/Tool Spawn from the GUI (Plan #6)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Let the GUI app (Wasmtime) run real `.wasm` tools in a terminal window:
host functions to open a PTY, spawn a wasmi tool on its slave, and poll the tool's
exit — while the GUI reads/writes the master via ordinary WASI `fd_read`/
`fd_write`. The terminal emulator itself (vte → cells → egui) lives PC-side; this
plan is the kernel bridge.

**Architecture:** `ruos_proc` host fns on the `Linker<WtState>` (Plan #7):
`pty_open` allocates a master/slave pair and installs the master as a `WtFd::Pty`
in the GUI's fd table; `proc_spawn` launches a wasmi `Fiber` bound to the slave
pts as a concurrent executor task (the same mechanism `ssh_spawn.rs` uses to bind
an SSH channel to a shell); `proc_poll` reports liveness/exit from the proc
registry. Tools run on **wasmi**; the GUI runs on **Wasmtime**; the PTY is the
only contact point. This is the "channel 2" of the spec (interactive command
execution); native desktop ops use direct WASI fns (channel 1).

**Depends on:** Plan #7 (`Linker<WtState>`, `WtState.fds`, `WtFd::Pty`, WASI
`fd_read`/`fd_write`), existing `crate::pty`, `crate::wasm::{exec_queue,
ssh_spawn}`, `crate::proc`.

**Build/run via WSL** (memory `ruos-build-env`); verify with boot-checks + QEMU
`-cpu max`.

> Before coding, confirm the real APIs:
> `grep -n "pub fn" kernel/src/pty/mod.rs` (alloc pair, master/slave push/pull,
> winsize), and read `kernel/src/wasm/ssh_spawn.rs` for the exact spawn-on-PTY
> pattern, and `kernel/src/proc.rs` for `register`/`unregister`/lookup.

---

## File Structure

- Create: `kernel/src/wasm/wt/proc_abi.rs` — `ruos_proc` host fns on the Linker.
- Modify: `kernel/src/wasm/wt/state.rs` — helper to install a master fd.
- Modify: `kernel/src/wasm/wt/mod.rs` — `proc_abi::add_to_linker` in `run_cwasm`.
- Possibly add a small spawn helper in `kernel/src/wasm/exec_queue.rs`.

---

## Task 1: PTY pair allocation API (confirm/extend)

**Files:** Inspect/extend `kernel/src/pty/mod.rs`.

- [ ] **Step 1: Confirm the PTY API**

`grep -n "pub fn" kernel/src/pty/mod.rs`. Identify: allocate a free pair →
`(master_handle, slave_pts_index)`; push bytes to master input
(`master_input_push`); read slave output / push slave output
(`slave_output_push`); read master output (what the GUI reads); set winsize. The
keyboard ISR already calls `crate::pty::master_input_push(0, b)` and WASI
`fd_write` to a pts calls a slave-output push — reuse those.

- [ ] **Step 2: Add `pty_alloc()` if missing**

If there's no "allocate a free pair" call, add:

```rust
/// Allocate a free PTY pair. Returns (master_id, slave_pts_index) or None.
pub fn pty_alloc() -> Option<(usize, usize)> { /* find a free pair in the pool */ }
```

following the existing pool structure (the boot log shows "pty 4 pairs ready").

- [ ] **Step 3: Build; commit**

```bash
git commit -am "feat(pty): pty_alloc() for on-demand pair allocation"
```

---

## Task 2: `ruos_proc` host fns

**Files:** Create `kernel/src/wasm/wt/proc_abi.rs`; modify `kernel/src/wasm/wt/state.rs`, `kernel/src/wasm/wt/mod.rs`.

- [ ] **Step 1: Add a fd-install helper to `WtState`**

```rust
impl WtState {
    /// Install a fd backed by a PTY master, returning the guest fd number.
    pub fn install_pty_master(&mut self, master_id: usize) -> i32 {
        self.fds.push(WtFd::Pty(master_id));
        (self.fds.len() - 1) as i32
    }
}
```

(If master and slave need distinct WtFd variants for read/write direction, add
`WtFd::PtyMaster(usize)`; reads pull master-output, writes push master-input.
Confirm against the PTY API direction semantics from Task 1.)

- [ ] **Step 2: Implement the host fns**

```rust
//! `ruos_proc`: open PTYs and spawn wasmi tools for the GUI terminal.
use wasmtime::Linker;
use crate::wasm::wt::state::WtState;
use crate::wasm::wt::mem;

pub fn add_to_linker(linker: &mut Linker<WtState>) -> wasmtime::Result<()> {
    // pty_open(out_ptr) -> errno; writes { master_fd:i32, slave_id:i32 }.
    linker.func_wrap("ruos_proc", "pty_open",
        |mut caller: wasmtime::Caller<'_, WtState>, out: i32| -> i32 {
            let (master_id, slave) = match crate::pty::pty_alloc() { Some(p) => p, None => return 28 };
            let fd = caller.data_mut().install_pty_master(master_id);
            let mut buf = [0u8; 8];
            buf[0..4].copy_from_slice(&fd.to_le_bytes());
            buf[4..8].copy_from_slice(&(slave as i32).to_le_bytes());
            if mem::write(&mut caller, out as u32, &buf) { 0 } else { 28 }
        })?;

    // proc_spawn(path_ptr, path_len, argv_ptr, argv_len, slave_id, out_pid) -> errno
    linker.func_wrap("ruos_proc", "proc_spawn",
        |mut caller: wasmtime::Caller<'_, WtState>, path_ptr: i32, path_len: i32,
         argv_ptr: i32, argv_len: i32, slave_id: i32, out_pid: i32| -> i32 {
            let path = match mem::read(&mut caller, path_ptr as u32, path_len as u32) {
                Some(b) => b, None => return 28 };
            let argv_blob = match mem::read(&mut caller, argv_ptr as u32, argv_len as u32) {
                Some(b) => b, None => return 28 };
            // argv encoding: NUL-separated UTF-8 (mirror PC abi crate).
            let argv: alloc::vec::Vec<alloc::vec::Vec<u8>> =
                argv_blob.split(|&b| b == 0).filter(|s| !s.is_empty()).map(|s| s.to_vec()).collect();
            let path_s = match core::str::from_utf8(&path) { Ok(s) => s, Err(_) => return 28 };
            // Spawn a wasmi tool bound to the slave pts as a concurrent task.
            let pid = match crate::wasm::exec_queue::spawn_on_pts(path_s, argv, slave_id as usize) {
                Some(pid) => pid, None => return 2, // ENOENT
            };
            if mem::write(&mut caller, out_pid as u32, &(pid as i32).to_le_bytes()) { 0 } else { 28 }
        })?;

    // proc_poll(pid, out_ptr) -> errno; writes { exited:i32(bool), code:i32 }.
    linker.func_wrap("ruos_proc", "proc_poll",
        |mut caller: wasmtime::Caller<'_, WtState>, pid: i32, out: i32| -> i32 {
            let (exited, code) = crate::proc::poll_exit(pid as usize); // (bool, i32)
            let mut buf = [0u8; 8];
            buf[0..4].copy_from_slice(&(exited as i32).to_le_bytes());
            buf[4..8].copy_from_slice(&code.to_le_bytes());
            if mem::write(&mut caller, out as u32, &buf) { 0 } else { 28 }
        })?;

    // pty_set_winsize(master_fd, cols, rows) -> errno
    linker.func_wrap("ruos_proc", "pty_set_winsize",
        |caller: wasmtime::Caller<'_, WtState>, master_fd: i32, cols: i32, rows: i32| -> i32 {
            if let Some(WtFd::Pty(id)) = caller.data().fds.get(master_fd as usize) {
                crate::pty::set_winsize(*id, cols as u16, rows as u16);
                0
            } else { 8 } // EBADF
        })?;
    Ok(())
}
```

- [ ] **Step 3: Wire into `run_cwasm`** — add `proc_abi::add_to_linker(&mut linker)?;`.

- [ ] **Step 4: Build; commit**

```bash
git commit -am "feat(wt): ruos_proc host fns (pty_open/proc_spawn/proc_poll/winsize)"
```

---

## Task 3: `spawn_on_pts` + `proc::poll_exit` (the spawn mechanism)

**Files:** Modify `kernel/src/wasm/exec_queue.rs`, `kernel/src/proc.rs`.

- [ ] **Step 1: Implement `spawn_on_pts`**

Following `ssh_spawn.rs` (binds a shell to a PTY), add a function that loads
`/bin/<path>.wasm`, builds a wasmi `Fiber` with stdio bound to `slave` pts, sets
argv, registers a pid, and schedules the fiber as a concurrent executor task
(non-blocking — returns the pid immediately). Reuse the executor spawn the boot
shell uses.

```rust
/// Load /bin/<name>.wasm, bind stdio to `slave` pts, run as a background task.
/// Returns the pid, or None if the tool is not found.
pub fn spawn_on_pts(name: &str, argv: Vec<Vec<u8>>, slave: usize) -> Option<usize> {
    // resolve path (/bin then /mnt/bin), read bytes (async → use the executor
    // spawn that run_boot_shell uses), Fiber::new, bind pts, register pid,
    // spawn task that runs fb.run().await then proc::mark_exited(pid, code).
    todo!("port from ssh_spawn.rs / run_boot_shell")
}
```

(This is the one substantial piece — model it exactly on how `ssh_spawn.rs` spawns
the shell on a PTY and how `run_boot_shell` registers/unregisters a pid.)

- [ ] **Step 2: Add exit tracking to `proc`**

Extend `kernel/src/proc.rs` so a finished task records its exit code and
`poll_exit(pid) -> (bool exited, i32 code)` can be queried (today it only
register/unregister). Store a small `BTreeMap<pid, Option<i32>>` of exit codes.

- [ ] **Step 3: Build; commit**

```bash
git commit -am "feat(exec): spawn_on_pts + proc exit tracking"
```

---

## Task 4: End-to-end smoke (kernel-side)

- [ ] **Step 1:** Boot-check (or scripted) test: from a kernel test path, allocate
  a PTY, `spawn_on_pts("echo", ["echo","hi"], slave)`, read master output, assert
  it contains "hi"; `poll_exit` eventually reports exited. Log `proc spawn e2e
  ok/FAIL`. (This exercises the bridge without the GUI app.)
- [ ] **Step 2:** Build, run under QEMU `-cpu max`, confirm; commit.

---

## Task 5: Changelog

- [ ] Create `CHANGELOG/NN-26-06-04-ruos-proc-terminal.md`; commit.

---

## Self-Review notes

- **Spec coverage:** implements spec §3.1 (two-runtime PTY bridge) and §5.1
  (`ruos_proc`: pty_open/proc_spawn/proc_poll/winsize). The vte terminal emulator
  is PC-side (gui-core), out of this plan.
- **Biggest unknown:** `spawn_on_pts` (Task 3) — depends on the exact executor
  spawn + PTY-bind pattern in `ssh_spawn.rs`/`run_boot_shell`. Read those first;
  the `todo!()` must be replaced with the real port before building.
- **ABI sync:** argv encoding (NUL-separated) and the `pty_open`/`proc_poll`
  out-struct layouts must match the PC `abi` crate.
- **Verify names:** `crate::pty::{pty_alloc,set_winsize}`, `crate::proc::poll_exit`,
  `crate::wasm::exec_queue::spawn_on_pts` are introduced by this plan — confirm the
  surrounding modules' real shapes before wiring.
- **Concurrency:** spawned wasmi fibers and the Wasmtime GUI both run cooperatively
  on the BSP; the GUI's `fd_read` on the master must epoch-yield (Plan #7 Task 6)
  so a spawned tool gets CPU to produce output.
```
