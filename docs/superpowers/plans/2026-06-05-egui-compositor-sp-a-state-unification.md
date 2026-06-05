# Compositor egui SP-A — State Unification + WASI on the compositor linker — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **Spec:** `docs/superpowers/specs/2026-06-05-egui-compositor-sp-a-state-unification-design.md` (read it first).

**Goal:** Make a `wasm32-wasip1` (std) reactor guest spawn from the compositor launcher and composite as a window, by registering BOTH WASI Preview 1 and the `wm` surface protocol onto one `Linker<AppState>` — without breaking the existing command-app path (`run_cwasm`, `Linker<WtState>`).

**Architecture:** Make the two host-fn registrations generic over accessor traits (`HasWasi`/`HasWindow`). Keep `WtState`/`WmState` as-is; define `AppState { wasi: WtState, win: WmState }` that implements both. The compositor builds one `Linker<AppState>` calling both generic `add_to_linker`s. `run_cwasm` keeps `Linker<WtState>` (since `WtState: HasWasi`). A trivial wasip1 reactor proves the path. No egui yet.

**Tech Stack:** Rust pinned nightly, kernel `no_std`, target `x86_64-unknown-none`, build-std via WSL. wasmtime 45 core (`Linker`/`Store`/`Module` AOT). Guest: `wasm32-wasip1` reactor (`cdylib`, exports `frame`, no `_start`), precompiled via `tools/wt-precompile`. Verification: kernel compile (the type system is the test for the generic refactor) + headless boot-check (`make test-boot`) + QEMU+KVM QMP screendump + VBox (`[[vbox-test-harness]]`).

---

## The uniform mechanical transform (applies in Tasks 3 and 5)

This plan turns concrete-typed host closures into trait-generic ones. The transform is **uniform** — the compiler enforces completeness (any closure left on the old concrete `caller.data()` type fails to type-check against `Caller<'_, T>`):

- A closure `|mut caller: Caller<'_, WtState>, ...|` becomes `|mut caller: Caller<'_, T>, ...|` (with `T: HasWasi` on the enclosing `add_to_linker<T>`).
- Every `caller.data().<field-or-method>` (read) → `caller.data().wasi_ref().<field-or-method>`.
- Every `caller.data_mut().<field-or-method>` (write) → `caller.data_mut().wasi().<field-or-method>`.
- `mem::read/write/write_u32(&mut caller, ...)` are unchanged at call sites (they become generic over `T` in Task 2, so they accept `Caller<'_, T>`).
- The `WtFd` enum and `WtState::{get, install_vfs}` are unchanged; access them through the accessor (`caller.data().wasi_ref().get(fd)`, `caller.data_mut().wasi().install_vfs(f)`).

For `wm.rs` (Task 5) the same transform uses `HasWindow`/`win()`/`win_ref()` instead.

---

## File Structure

| File | Responsibility after SP-A |
|---|---|
| `kernel/src/wasm/wt/state.rs` | `WtState` (unchanged fields) + `HasWasi` trait + `impl HasWasi for WtState`. |
| `kernel/src/wasm/wt/mem.rs` | `read/write/write_u32` generic over `T` (memory-export only; no state fields touched). |
| `kernel/src/wasm/wt/wasi.rs` | `add_to_linker<T: HasWasi>(&mut Linker<T>)`; all closures `Caller<'_, T>` via the accessor. |
| `kernel/src/wasm/wt/wm.rs` | `HasWindow` trait + `impl for WmState`; `AppState { wasi, win }` (impl both); `add_to_linker<T: HasWindow>`; `Compositor` on `Store<AppState>`/`Linker<AppState>`; compositor linker = wasi+wm; `read_guest`/`write_guest` generic (or replaced by `mem`). |
| `kernel/src/wasm/wt/mod.rs` | `run_cwasm` unchanged in behaviour (compiles because `WtState: HasWasi`). |
| `tools/wt-wasip1-probe/{Cargo.toml,src/lib.rs}` | NEW. wasip1 reactor: `frame()` does a std alloc + fills a 320×240 RGBA buffer + `wm.commit`. |
| `Makefile` | Build `probe.cwasm` (wasip1 → wt-precompile) + ship + prereq. |
| `kernel/src/wasm/wt/wm.rs` (APPS) | Add `AppEntry { name: "wasip1-probe", cwasm: PROBE_CWASM }`. |
| `kernel/src/wasm/wt/mod.rs` + `boot/phases/interrupts.rs` | Headless boot-check `probe spawn ok`. |

---

## Task 1: `HasWasi` accessor trait + impl for `WtState`

**Files:** Modify `kernel/src/wasm/wt/state.rs`.

- [ ] **Step 1: Add the trait + self-impl.** Append to `kernel/src/wasm/wt/state.rs`:

```rust
/// Capability accessor: any Store-data type that carries a `WtState` (the WASI
/// state) exposes it here, so `wasi::add_to_linker` can be generic over the
/// store-data type instead of hard-wired to `WtState`. `WtState` itself is the
/// trivial holder (returns `self`); `AppState` (compositor windows) returns its
/// embedded `wasi` field.
pub trait HasWasi {
    fn wasi(&mut self) -> &mut WtState;
    fn wasi_ref(&self) -> &WtState;
}

impl HasWasi for WtState {
    fn wasi(&mut self) -> &mut WtState { self }
    fn wasi_ref(&self) -> &WtState { self }
}
```

- [ ] **Step 2: Build the kernel — confirms the trait compiles, nothing else changed.**

```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem/kernel && source $HOME/.cargo/env && cargo build --release -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem --target x86_64-unknown-none 2>&1 | tail -8'
```
Expected: `Finished`. A `dead_code`/`unused` warning on `HasWasi`/`wasi_ref` is fine (wired in Task 3).

- [ ] **Step 3: Commit.**

```bash
cd /e/MinimalOS/BasicOperatingSystem && git add kernel/src/wasm/wt/state.rs && git commit -m "feat(wt): HasWasi accessor trait + impl for WtState"
```

---

## Task 2: Make `mem.rs` generic over the store-data type

`mem.rs` only ever uses the guest's `memory` export (never a `WtState` field), so it can be generic over any `T`. This lets the Task-3 WASI closures (now `Caller<'_, T>`) keep calling `mem::read/write/write_u32`.

**Files:** Modify `kernel/src/wasm/wt/mem.rs`.

- [ ] **Step 1: Drop the `WtState` import and genericise all four fns.** Replace the whole body of `kernel/src/wasm/wt/mem.rs` with:

```rust
//! The single audited path to a Wasmtime guest's linear memory (mirrors the
//! wasmi `wasm/host/mem.rs` rule: no raw guest reads/writes elsewhere).
//! Generic over the Store data type `T` — it only touches the `memory` export.

use wasmtime::{Caller, Extern, Memory};
use alloc::vec::Vec;

fn memory<T>(caller: &mut Caller<'_, T>) -> Option<Memory> {
    match caller.get_export("memory") {
        Some(Extern::Memory(m)) => Some(m),
        _ => None,
    }
}

/// Copy `buf` into guest memory at `ptr`. Returns false if out of bounds.
pub fn write<T>(caller: &mut Caller<'_, T>, ptr: u32, buf: &[u8]) -> bool {
    match memory(caller) {
        Some(mem) => mem.write(caller, ptr as usize, buf).is_ok(),
        None => false,
    }
}

/// Read `len` bytes from guest memory at `ptr`. None if out of bounds.
pub fn read<T>(caller: &mut Caller<'_, T>, ptr: u32, len: u32) -> Option<Vec<u8>> {
    let mem = memory(caller)?;
    let mut out = alloc::vec![0u8; len as usize];
    mem.read(caller, ptr as usize, &mut out).ok()?;
    Some(out)
}

/// Write a little-endian u32 to guest memory. False if out of bounds.
pub fn write_u32<T>(caller: &mut Caller<'_, T>, ptr: u32, val: u32) -> bool {
    write(caller, ptr, &val.to_le_bytes())
}
```

- [ ] **Step 2: Build — `wasi.rs` still calls `mem::*` with `Caller<'_, WtState>`, which now matches `T = WtState`.**

```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem/kernel && source $HOME/.cargo/env && cargo build --release -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem --target x86_64-unknown-none 2>&1 | tail -8'
```
Expected: `Finished` (type inference fills `T = WtState` at the existing call sites).

- [ ] **Step 3: Commit.**

```bash
cd /e/MinimalOS/BasicOperatingSystem && git add kernel/src/wasm/wt/mem.rs && git commit -m "refactor(wt): make mem read/write generic over Store data type"
```

---

## Task 3: Genericise `wasi::add_to_linker` over `HasWasi`

The big mechanical task. Apply the **uniform transform** (top of this plan) to every closure in `wasi.rs`.

**Files:** Modify `kernel/src/wasm/wt/wasi.rs`.

- [ ] **Step 1: Change the function signature.** In `kernel/src/wasm/wt/wasi.rs`:

```rust
// BEFORE:  pub fn add_to_linker(linker: &mut Linker<WtState>) -> wasmtime::Result<()> {
// AFTER:
pub fn add_to_linker<T: HasWasi + 'static>(linker: &mut Linker<T>) -> wasmtime::Result<()> {
```
Add the import: `use crate::wasm::wt::state::HasWasi;` (keep `use crate::wasm::wt::state::{WtState, WtFd};` — `WtFd` is still matched, `WtState` is still the accessor return type).

- [ ] **Step 2: Transform every closure** per the uniform rule. Each `|mut caller: Caller<'_, WtState>, ...|` → `|mut caller: Caller<'_, T>, ...|`; each `caller.data().X` → `caller.data().wasi_ref().X`; each `caller.data_mut().X` → `caller.data_mut().wasi().X`. The closures to transform (all in `wasi.rs`): `proc_exit`, `fd_write`, `fd_read`, `fd_seek`, `fd_close`, `fd_fdstat_get`, `fd_filestat_get`, `fd_prestat_get`, `fd_prestat_dir_name`, `path_open`, `args_sizes_get`, `args_get`, `environ_sizes_get`, `environ_get`, `clock_time_get`, `random_get`, `sched_yield`. Two worked templates (apply the same shape to the rest):

```rust
    // proc_exit: write goes through wasi()
    linker.func_wrap("wasi_snapshot_preview1", "proc_exit",
        |mut caller: Caller<'_, T>, code: i32| -> wasmtime::Result<()> {
            caller.data_mut().wasi().exit = Some(code);
            // (keep the existing trap/return-from-_start behaviour exactly as before)
            // ... existing body unchanged except the data_mut() hop ...
        })?;

    // args_sizes_get: reads go through wasi_ref()
    linker.func_wrap("wasi_snapshot_preview1", "args_sizes_get",
        |mut caller: Caller<'_, T>, argc: i32, buf_size: i32| -> i32 {
            let n = caller.data().wasi_ref().args.len() as u32;
            let sz: u32 = caller.data().wasi_ref().args.iter().map(|a| a.len() as u32 + 1).sum();
            if !mem::write_u32(&mut caller, argc as u32, n) { return EINVAL; }
            if !mem::write_u32(&mut caller, buf_size as u32, sz) { return EINVAL; }
            OK
        })?;
```
Note for `fd_read`/`path_open` etc.: `caller.data().get(fd)` → `caller.data().wasi_ref().get(fd)`; `caller.data_mut().install_vfs(vfd)` → `caller.data_mut().wasi().install_vfs(vfd)`; `caller.data().stdout_pty` → `caller.data().wasi_ref().stdout_pty`; `caller.data_mut().fds.get_mut(..)` → `caller.data_mut().wasi().fds.get_mut(..)`. **Do not change any logic** — only the type and the accessor hop. Watch the borrow rule already in the file (don't hold a `data_mut()` borrow across a `mem::read` — the existing code already avoids this; preserve it).

- [ ] **Step 3: Build — `run_cwasm` (Task uses it on `Linker<WtState>`) must still compile because `WtState: HasWasi`.**

```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem/kernel && source $HOME/.cargo/env && cargo build --release -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem --target x86_64-unknown-none 2>&1 | tail -20'
```
Expected: `Finished`. If a closure was missed, the error is a type mismatch (`Caller<'_, WtState>` vs `Caller<'_, T>`) pointing at the exact closure — fix it. (This is the completeness check.)

- [ ] **Step 4: Commit.**

```bash
cd /e/MinimalOS/BasicOperatingSystem && git add kernel/src/wasm/wt/wasi.rs && git commit -m "refactor(wt): genericise wasi::add_to_linker over HasWasi"
```

---

## Task 4: `HasWindow` trait + `AppState` (no behaviour change yet)

**Files:** Modify `kernel/src/wasm/wt/wm.rs`.

- [ ] **Step 1: Add the trait + `AppState`.** Near the top of `kernel/src/wasm/wt/wm.rs` (after the `WmState` definition), add:

```rust
use crate::wasm::wt::state::{WtState, HasWasi};

/// Capability accessor for the window/surface state (mirror of HasWasi).
pub trait HasWindow {
    fn win(&mut self) -> &mut WmState;
    fn win_ref(&self) -> &WmState;
}

impl HasWindow for WmState {
    fn win(&mut self) -> &mut WmState { self }
    fn win_ref(&self) -> &WmState { self }
}

/// A compositor window's Store data: BOTH the WASI capability (so a wasip1 egui
/// guest's std runtime links + runs) AND the window/surface state. Embeds the
/// existing structs unchanged; implements both accessor traits.
pub struct AppState {
    pub wasi: WtState,
    pub win: WmState,
}

impl HasWasi for AppState {
    fn wasi(&mut self) -> &mut WtState { &mut self.wasi }
    fn wasi_ref(&self) -> &WtState { &self.wasi }
}
impl HasWindow for AppState {
    fn win(&mut self) -> &mut WmState { &mut self.win }
    fn win_ref(&self) -> &WmState { &self.win }
}
```

- [ ] **Step 2: Build (trait/struct unused yet → warnings OK).**

```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem/kernel && source $HOME/.cargo/env && cargo build --release -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem --target x86_64-unknown-none 2>&1 | tail -8'
```
Expected: `Finished`.

- [ ] **Step 3: Commit.**

```bash
cd /e/MinimalOS/BasicOperatingSystem && git add kernel/src/wasm/wt/wm.rs && git commit -m "feat(wm): HasWindow trait + AppState (wasi + win)"
```

---

## Task 5: Genericise `wm::add_to_linker` over `HasWindow`

**Files:** Modify `kernel/src/wasm/wt/wm.rs`.

- [ ] **Step 1: Change the signature + the 5 closures.** Apply the uniform transform with `HasWindow`/`win()`/`win_ref()`:

```rust
// BEFORE:  pub fn add_to_linker(linker: &mut Linker<WmState>) -> wasmtime::Result<()> {
// AFTER:
pub fn add_to_linker<T: HasWindow + 'static>(linker: &mut Linker<T>) -> wasmtime::Result<()> {
```
The 5 closures (`wm.commit`, `wm.app_id`, `wm.tick`, `wm.poll_event`, `wm.close`) become `Caller<'_, T>`; their `caller.data()/data_mut()` accesses to `WmState` fields (`pixels`, `win_w`, `win_h`, `id`, `tick`, `events`, `close_requested`) go through `win_ref()`/`win()`.

- [ ] **Step 2: Make the private `read_guest`/`write_guest` helpers generic** (they currently take `Caller<'_, WmState>`). Either (a) change their signature to `<T>(caller: &mut Caller<'_, T>, ...)` (they only use the `memory` export, like `mem.rs`), OR (b) delete them and call `crate::wasm::wt::mem::{read, write}` directly (now generic). Prefer (b) to DRY — replace `read_guest(&mut caller, p, l)` with `crate::wasm::wt::mem::read(&mut caller, p, l)` and `write_guest(&mut caller, p, &buf)` with `crate::wasm::wt::mem::write(&mut caller, p, &buf)`, then delete the two helpers.

- [ ] **Step 3: Build — `Compositor` still uses `WmState` (which impls `HasWindow`), so `wm::add_to_linker` type-checks on `Linker<WmState>` for now.**

```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem/kernel && source $HOME/.cargo/env && cargo build --release -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem --target x86_64-unknown-none 2>&1 | tail -15'
```
Expected: `Finished`.

- [ ] **Step 4: Commit.**

```bash
cd /e/MinimalOS/BasicOperatingSystem && git add kernel/src/wasm/wt/wm.rs && git commit -m "refactor(wm): genericise wm::add_to_linker over HasWindow; DRY guest mem via mem.rs"
```

---

## Task 6: Switch the `Compositor` to `Store<AppState>` / `Linker<AppState>`

Now make windows carry `AppState` and register BOTH worlds on the compositor linker. The existing solid-colour reactors (which import only `wm`) still instantiate fine against a linker that ALSO offers WASI (extra available imports are harmless).

**Files:** Modify `kernel/src/wasm/wt/wm.rs`.

- [ ] **Step 1: Re-type the `Compositor` + its constructors.** Change every `Store<WmState>` → `Store<AppState>` and `Linker<WmState>` → `Linker<AppState>` in `Window`, `Compositor`, `Compositor::new`, `Compositor::new_empty`, `run_reactor_spike` (if it builds a window-style store), and `spawn_app`. Construct the store data as:

```rust
let mut store = Store::new(
    engine,
    AppState {
        wasi: WtState::new(alloc::vec![b"win".to_vec()]),
        win:  WmState { id, win_w: 0, win_h: 0, pixels: Vec::new(), tick: 0, events: VecDeque::new(), close_requested: false },
    },
);
```
(Use the EXACT current `WmState { .. }` field list from the file; only wrap it in `AppState { wasi: WtState::new(...), win: <that> }`. `WtState::new` needs a non-empty argv so std's `args()` is happy — `b"win"` is fine.)

- [ ] **Step 2: Build the compositor linker with BOTH registrations.** Where `Compositor::new`/`new_empty` builds the linker, register wasi THEN wm:

```rust
let mut linker: Linker<AppState> = Linker::new(engine);
crate::wasm::wt::wasi::add_to_linker(&mut linker).expect("wasi linker");
add_to_linker(&mut linker).expect("wm linker"); // wm::add_to_linker (this module)
```
(`add_to_linker::<AppState>` resolves because `AppState: HasWindow`; `wasi::add_to_linker::<AppState>` resolves because `AppState: HasWasi`.)

- [ ] **Step 3: Fix every `store.data()`/`store.data_mut()` site in the compositor body** (not host closures — the kernel-side reads like `w.store.data().pixels` in `compose_window`, `present`, `reap`, `frame_all`) to go through `.win` (e.g. `w.store.data().win.pixels`, `w.store.data().win.close_requested`). The compiler lists each site; change `.<field>` → `.win.<field>` for window fields. (These are direct field accesses on `AppState`, not the trait — use `.win.` directly.)

- [ ] **Step 4: Build.**

```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem/kernel && source $HOME/.cargo/env && cargo build --release -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem --target x86_64-unknown-none 2>&1 | tail -20'
```
Expected: `Finished`. Also build `--features boot-checks` and `--features serial-composite` → `Finished`.

- [ ] **Step 5: Regression boot-check — the existing solid reactors still composite.** Build + boot the compositor ISO and confirm the GATE/SP5 markers still fire (the unknown-unknown reactors run against the now-WASI-bearing linker):

```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make test-boot ISO=build/cmtest.iso 2>&1 | tail -8 && grep -E "launcher registry apps=|lifecycle spawns=1 peak_live=1 final_live=0" build/test-boot.log'
```
Expected: the SP5 lifecycle markers still match (existing reactors unaffected).

- [ ] **Step 6: Commit.**

```bash
cd /e/MinimalOS/BasicOperatingSystem && git add kernel/src/wasm/wt/wm.rs && git commit -m "feat(wm): compositor on Store<AppState>/Linker<AppState> (WASI + wm on one linker)"
```

---

## Task 7: The wasip1 probe guest + Makefile + APPS registration

**Files:** Create `tools/wt-wasip1-probe/{Cargo.toml, src/lib.rs}`; modify `Makefile`, `kernel/src/wasm/wt/wm.rs`.

- [ ] **Step 1: Guest crate `tools/wt-wasip1-probe/Cargo.toml`** (std + wasip1 + reactor cdylib):

```toml
[package]
name = "wt-wasip1-probe"
version = "0.0.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[profile.release]
panic = "abort"
lto = true
```

- [ ] **Step 2: Guest `tools/wt-wasip1-probe/src/lib.rs`** — a STD reactor (no `#![no_std]`): exports `frame()`, does a heap alloc to prove std works, fills a 320×240 RGBA buffer, commits via `wm`:

```rust
//! wasip1 STD reactor probe (SP-A). Proves a wasm32-wasip1 std guest runs as a
//! compositor window: it allocates on the heap (std), fills its surface, and
//! commits via `wm`. No egui — this only exercises WASI + wm on one linker.

#[link(wasm_import_module = "wm")]
extern "C" {
    fn commit(ptr: *const u8, len: u32, w: u32, h: u32);
    fn app_id() -> u32;
    fn tick();
}

const W: usize = 320;
const H: usize = 240;

#[no_mangle]
pub extern "C" fn frame() {
    unsafe { tick(); }
    let id = unsafe { app_id() };
    // STD heap allocation (the whole point: proves the std/libc/WASI runtime works).
    let mut buf: Vec<u8> = Vec::with_capacity(W * H * 4);
    // A solid colour that depends on app_id, like the no_std reactor, so the
    // window is visibly distinct.
    let (r, g, b) = (0x30u8, 0x80u8.wrapping_add((id as u8).wrapping_mul(40)), 0xB0u8);
    for _ in 0..(W * H) {
        buf.push(r); buf.push(g); buf.push(b); buf.push(0xFF);
    }
    unsafe { commit(buf.as_ptr(), (W * H * 4) as u32, W as u32, H as u32); }
}
```
(No `main`/`_start`: a `cdylib` with only a `#[no_mangle] frame` export builds as a wasip1 *reactor* — wasm-ld emits the `wm` + `wasi_snapshot_preview1` imports std references. If the toolchain still emits a `_start`, add a `#[no_mangle] pub extern "C" fn _start() {}` no-op so instantiation doesn't require running it — the compositor never calls `_start`.)

- [ ] **Step 3: Makefile rule** (mirror the `reactor.cwasm` rule near line 151, but `wasm32-wasip1`):

```makefile
# wasip1 STD reactor probe (egui SP-A): proves a std/wasip1 guest runs as a
# compositor window. Built wasm32-wasip1 (std), precompiled to a CORE .cwasm.
kernel/src/wasm/wt/probe.cwasm: tools/wt-wasip1-probe/src/lib.rs tools/wt-wasip1-probe/Cargo.toml $(WT_PRECOMPILE)
	@mkdir -p build
	source $$HOME/.cargo/env && cd tools/wt-wasip1-probe && \
		cargo build --release --target wasm32-wasip1
	$(WT_PRECOMPILE) tools/wt-wasip1-probe/target/wasm32-wasip1/release/wt_wasip1_probe.wasm kernel/src/wasm/wt/probe.cwasm
```
Add `kernel/src/wasm/wt/probe.cwasm` as a prerequisite to BOTH the `iso:` and `test-boot:` targets (next to `reactor.cwasm`), and `cp kernel/src/wasm/wt/probe.cwasm $(ISO_ROOT)/bin/probe.cwasm` in both recipes (optional shipping; the kernel embeds it).

- [ ] **Step 4: Register in `APPS`** (`kernel/src/wasm/wt/wm.rs`): add the embed + entry:

```rust
static PROBE_CWASM: &[u8] = include_bytes!("probe.cwasm");
// ... in `pub static APPS: &[AppEntry] = &[ ... ];` add:
    AppEntry { name: "wasip1-probe", cwasm: PROBE_CWASM },
```

- [ ] **Step 5: Build the probe + kernel.**

```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && source $HOME/.cargo/env && make kernel/src/wasm/wt/probe.cwasm 2>&1 | tail -6 && wasm-tools print tools/wt-wasip1-probe/target/wasm32-wasip1/release/wt_wasip1_probe.wasm | grep -E "import \"(wm|wasi_snapshot_preview1)\"|export .*frame" | head -20'
```
Expected: `wrote …probe.cwasm`; imports include `wm.commit/app_id/tick` AND several `wasi_snapshot_preview1.*`; exports `frame`. Then build the kernel (`cargo build --release ... --target x86_64-unknown-none`) → `Finished` (the `include_bytes!` now resolves).

- [ ] **Step 6: Commit.**

```bash
cd /e/MinimalOS/BasicOperatingSystem && git add tools/wt-wasip1-probe Makefile kernel/src/wasm/wt/wm.rs && git commit -m "feat(wm): wasip1 STD probe guest + APPS entry + Makefile rule"
```

---

## Task 8: Headless boot-check — the probe instantiates + commits against `Linker<AppState>`

**Files:** Modify `kernel/src/wasm/wt/wm.rs`, `kernel/src/wasm/wt/mod.rs`, `kernel/src/boot/phases/interrupts.rs`.

- [ ] **Step 1: A headless self-test in `wm.rs`** that builds a `Compositor::new_empty()`, spawns the probe, calls `frame()` once, and reports committed pixel length:

```rust
/// Boot self-test: spawn the wasip1 STD probe (registry index for "wasip1-probe"),
/// call frame() once, and report whether it committed a surface. Proves a
/// std/wasip1 guest instantiates + runs against the unified Linker<AppState>.
pub fn wasip1_probe_self_test() -> usize {
    let mut c = Compositor::new_empty();
    let idx = APPS.iter().position(|a| a.name == "wasip1-probe").unwrap_or(usize::MAX);
    if idx == usize::MAX { return 0; }
    if c.spawn_app(idx).is_none() { return 0; }
    c.frame_all();
    c.wins.last().map(|w| w.store.data().win.pixels.len()).unwrap_or(0)
}
```

- [ ] **Step 2: mod.rs wrapper + interrupts marker.** In `kernel/src/wasm/wt/mod.rs`:

```rust
#[cfg(feature = "boot-checks")]
pub fn run_wasip1_probe_demo() -> usize { crate::wasm::wt::wm::wasip1_probe_self_test() }
```
In `kernel/src/boot/phases/interrupts.rs` (boot-checks block, next to the other `wm` markers):

```rust
        let pn = crate::wasm::wt::run_wasip1_probe_demo();
        crate::binfo!("wm", "wasip1 probe spawn ok pixels={}", pn);
```

- [ ] **Step 3: Build + boot-check assert.**

```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make test-boot ISO=build/cmtest.iso 2>&1 | tail -8 && grep -E "wasip1 probe spawn ok pixels=307200" build/test-boot.log'
```
Expected: `wasip1 probe spawn ok pixels=307200` (320×240×4). Anything else: `pixels=0` = instantiate failed (missing WASI import → check `wasm-tools print` imports vs what `wasi::add_to_linker` registers) or frame()/commit failed. Report the exact line + the probe's import list.

- [ ] **Step 4: Commit.**

```bash
cd /e/MinimalOS/BasicOperatingSystem && git add kernel/src/wasm/wt/wm.rs kernel/src/wasm/wt/mod.rs kernel/src/boot/phases/interrupts.rs && git commit -m "test(wm): headless boot-check — wasip1 STD probe spawns + commits against Linker<AppState>"
```

---

## Task 9: Visual verification (QEMU+KVM, then VBox)

**Files:** none (reuse `user-bin/compositor-init.sh`, the QMP driver pattern from `build/comp_verify.py`).

- [ ] **Step 1: Build the compositor ISO.**

```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && source $HOME/.cargo/env && make iso INIT_SCRIPT=user-bin/compositor-init.sh ISO=build/comptest.iso 2>&1 | tail -3'
```

- [ ] **Step 2: QMP driver `build/probe_verify.py`** (model on `build/launch_verify.py`): boot headless, wait ~16s, screendump the initial desktop; the launcher now has a `wasip1-probe` button (4th entry, center ≈ (336, height-14) at BTN_W=96); move there + click; wait ~1.5s; screendump → a NEW window whose surface is the probe's colour appears (spawned from a std/wasip1 guest). Serial shows `spawn app='wasip1-probe' ... live=N`.

```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && rm -f /tmp/qmp.sock && (timeout 45 qemu-system-x86_64 -machine q35,accel=kvm:tcg -cpu max -m 512 -no-reboot -display none -serial file:build/probe-serial.log -qmp unix:/tmp/qmp.sock,server,nowait -device qemu-xhci -cdrom build/comptest.iso & sleep 1 && python3 build/probe_verify.py) 2>&1 | tail -10; grep -E "spawn app=.wasip1-probe" build/probe-serial.log || echo NO_SPAWN'
```
Expected: a `spawn app='wasip1-probe'` line + `build/probe-1-spawned.png` showing the probe window. (Send the screendump to the controller for review.)

- [ ] **Step 3: VBox sanity** (VM `ruos`, EFI + 6 vCPU; per `[[vbox-test-harness]]`): attach `build/comptest.iso`, boot headless, screenshot, confirm the probe window renders + serial shows the spawn; restore `os.iso`. (SP-A isn't SMP-sensitive but VBox confirms the std/wasip1 path on the HW-like target.)

- [ ] **Step 4 (if it fails):** STOP + report. `pixels=0`/no window = a WASI import the probe needs isn't registered (compare `wasm-tools print` imports vs `wasi.rs`); add the missing closure to `wasi.rs`. A trap on `frame()` = the std runtime hit an unsupported WASI call (check the serial for a trap) — note which, decide stub-vs-implement (likely SP-B refines this).

---

## Task 10: Changelog + final review

- [ ] **Step 1:** Write `CHANGELOG/NN-26-06-05-egui-compositor-sp-a.md` (next free `NN` — currently `283`, so `284` unless taken). Summarise: HasWasi/HasWindow accessor traits + `AppState`, generic `wasi::`/`wm::add_to_linker`, generic `mem.rs`, compositor on `Linker<AppState>`, the wasip1 STD probe proving WASI+wm coexist (`probe spawn ok pixels=307200` + screendump), `run_cwasm` unchanged. Reference the spec + `[[vbox-test-harness]]`.
- [ ] **Step 2:** Commit the changelog. Dispatch a final code-reviewer over `wasi.rs`/`wm.rs`/`state.rs`/`mem.rs` diffs focusing on: (a) no closure left on the concrete type, (b) no borrow-across-`mem::read` regressions, (c) the command-app path (`run_cwasm`) is behaviourally unchanged, (d) `AppState` construction wires `WtState::new` with a valid argv.

---

## Provides (for SP-B)

- `Linker<AppState>` (WASI + `wm`) + `Store<AppState>` per window — SP-B's egui-reactor instantiates against exactly this; `AppState.wasi`/`AppState.win` are the capability homes.
- The proven `wasm32-wasip1` reactor cargo shape (`cdylib`, `frame` export, optional no-op `_start`) + the `probe.cwasm` precompile path — SP-B's egui app reuses it.
- Documented hazard for SP-B: `proc_exit`/`frame()` trap in a persistent reactor (map to `close_requested` → reap) and the WASI subset egui actually needs (read from the probe's `wasm-tools print` import list).

## Self-Review notes
- **Spec coverage:** trait-generic state (spec §Architecture) = Tasks 1,4; WASI on the compositor linker = Tasks 2,3,5,6; `run_cwasm` unchanged = Tasks 3,6 build gates; probe + verification (spec §Testing) = Tasks 7–9; out-of-scope egui/proc_exit explicitly deferred. No spec requirement without a task.
- **Placeholders:** the mechanical-transform tasks (3,5) deliberately use the *uniform rule + two worked templates + the exact closure list* rather than pasting 17 near-identical closures — the compiler enforces completeness (a missed closure = a type error at that closure), which is the precise, non-vague completion check. All NEW code (traits, AppState, probe, boot-check) is shown in full.
- **Type consistency:** `HasWasi::{wasi,wasi_ref}`, `HasWindow::{win,win_ref}`, `AppState{wasi:WtState,win:WmState}`, `Linker<AppState>`, marker `wasip1 probe spawn ok pixels=307200` are used identically across tasks.
