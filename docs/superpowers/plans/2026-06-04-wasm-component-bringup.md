# WASM Component-Model Bring-up (Step 0) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove the wasmtime Component Model runs end-to-end in the ruos no_std AOT runtime by running a tiny `ruos:bringup` component (imports `system.log`/`poweroff`, exports `run`) as a boot self-test — establishing the whole toolchain pipeline (WIT → guest component → `precompile_component` → `Component::deserialize` + `component::Linker` + `bindgen!` host) before any desktop migration.

**Architecture:** Add a SECOND runtime entry point `run_component` in `kernel/src/wasm/wt/` alongside the existing `run_cwasm`, leaving all 23 existing host fns and the 50+ wasip1 tools untouched. One WIT package `ruos:bringup` is the single source of truth; the kernel implements its imports via the `bindgen!` macro; a tiny guest crate implements `run` via `wit-bindgen`. The component `.cwasm` is produced by `wt-precompile --component` (same fixed engine `Config` as the kernel) and exercised under the `boot-checks` feature, grepping a serial marker.

**Tech Stack:** Rust nightly-2026-05-26, `x86_64-unknown-none` (kernel) + `wasm32-wasip1` (guest); wasmtime 45.0.0 (`features=["runtime","custom-virtual-memory","component-model"]`); `wit-bindgen` (guest macro) + `wasm-tools` (component synthesis) + `wit-bindgen`/`bindgen!` (host); built via WSL Ubuntu (`make`).

**Verification idiom:** This layer has no unit-test harness; the repo's idiom is a **boot self-test** under `--features boot-checks` that emits a serial marker asserted by `make test-boot` (see `kernel/src/boot/phases/*` + `Makefile:349`). Each task's "test" is therefore *the build succeeds* and/or *the boot marker appears*. To avoid clobbering the real boot ISO (`build/os.iso`, the VBox medium), use `ISO=build/cmtest.iso` overrides for intermediate test-boots.

---

## File Structure (decomposition)

- `wit/ruos-bringup.wit` — **Create.** The WIT contract (package `ruos:bringup`, interface `system`, world `bringup`). Single source of truth.
- `tools/wt-bringup/` — **Create.** Guest crate: `Cargo.toml` + `src/lib.rs` (wit-bindgen `generate!`, exports `run`, imports `system`) + a trivial bump global allocator. Builds to a core wasm then a component.
- `tools/wt-precompile/src/main.rs` — **Modify.** Add a `--component` flag → `Engine::precompile_component` (same `Config` as today).
- `kernel/Cargo.toml` — **Modify.** Add `component-model` to the wasmtime feature list.
- `kernel/src/wasm/wt/component.rs` — **Create.** `bindgen!`-generated host bindings for `ruos:bringup` + `BringupHost` impl (`log`→`kprintln`, `poweroff`→`crate::power::poweroff`) + `run_component(cwasm) -> i32`.
- `kernel/src/wasm/wt/mod.rs` — **Modify.** `pub mod component;` + embed `bringup.cwasm` + `run_bringup_demo()` boot self-test (behind `boot-checks`).
- `kernel/src/boot/phases/interrupts.rs` — **Modify.** Wire `run_bringup_demo()` into the boot-checks block with a `binfo!` marker.
- `Makefile` — **Modify.** Build the guest component + precompile it to `build/bringup.cwasm`, copied next to the kernel sources for `include_bytes!`.

---

## Task 1: Enable `component-model` in the kernel (no_std), keep the build green

**Files:**
- Modify: `kernel/Cargo.toml:23`

- [ ] **Step 1: Add the feature**

In `kernel/Cargo.toml`, change the wasmtime line:

```toml
wasmtime = { version = "=45.0.0", default-features = false, features = ["runtime", "custom-virtual-memory", "component-model"] }
```

- [ ] **Step 2: Verify the kernel still builds no_std**

Run (WSL):
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && source $HOME/.cargo/env && cd kernel && cargo build --release -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem --target x86_64-unknown-none 2>&1 | tail -20'
```
Expected: `Finished \`release\`` (only the pre-existing dead-code warnings). A linker/`std` error here means component-model is NOT no_std-clean on the pinned toolchain — STOP and report (this contradicts the earlier `nightly`-generic compile test and must be resolved before continuing).

- [ ] **Step 3: Commit**

```bash
cd /e/MinimalOS/BasicOperatingSystem
git add kernel/Cargo.toml
git commit -m "build(wasm): enable wasmtime component-model feature (no_std)"
```

---

## Task 2: Install the WASM component toolchain in WSL

**Files:** none (environment).

- [ ] **Step 1: Install wasm-tools + wit-bindgen-cli**

Run (WSL):
```bash
wsl -d Ubuntu -u root -e bash -lc 'source $HOME/.cargo/env && cargo install wasm-tools wit-bindgen-cli 2>&1 | tail -5; wasm-tools --version; wit-bindgen --version'
```
Expected: both version lines print. Record the versions in the commit message of Task 4 (toolchain pinning matters: they must understand wasmtime 45's component encoding).

- [ ] **Step 2: Add the wasm32-wasip1 target to the pinned toolchain**

```bash
wsl -d Ubuntu -u root -e bash -lc 'source $HOME/.cargo/env && TC=$(grep -hoE "nightly-[0-9]{4}-[0-9]{2}-[0-9]{2}" /mnt/e/MinimalOS/BasicOperatingSystem/rust-toolchain* 2>/dev/null | head -1); rustup target add wasm32-wasip1 --toolchain ${TC:-nightly}; echo OK'
```
Expected: `OK` (target installed or already present).

No commit (environment only).

---

## Task 3: Author the WIT contract

**Files:**
- Create: `wit/ruos-bringup.wit`

- [ ] **Step 1: Write the WIT**

```wit
package ruos:bringup;

interface system {
  // Append a UTF-8 line to the kernel serial log (host side).
  log: func(msg: string);
  // Power the machine off. Never returns (the bring-up guest does NOT call this;
  // it exists to exercise a zero-arg, no-return import shape for later reuse).
  poweroff: func();
}

world bringup {
  import system;
  // Host calls this after instantiation; returns a status code.
  export run: func() -> s32;
}
```

- [ ] **Step 2: Validate it**

```bash
wsl -d Ubuntu -u root -e bash -lc 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && wasm-tools component wit wit/ruos-bringup.wit >/dev/null && echo WIT-OK'
```
Expected: `WIT-OK` (the WIT parses).

- [ ] **Step 3: Commit**

```bash
cd /e/MinimalOS/BasicOperatingSystem
git add wit/ruos-bringup.wit
git commit -m "feat(wasm): ruos:bringup WIT (system.log/poweroff + run)"
```

---

## Task 4: Build the guest bring-up component

**Files:**
- Create: `tools/wt-bringup/Cargo.toml`
- Create: `tools/wt-bringup/src/lib.rs`

- [ ] **Step 1: Guest Cargo.toml**

```toml
[package]
name = "wt-bringup"
version = "0.0.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
wit-bindgen = { version = "0.46", default-features = false, features = ["realloc"] }

[profile.release]
panic = "abort"
lto = true
```
(If `cargo install` reported a different wit-bindgen-cli major in Task 2, match the `wit-bindgen` crate major here — they must agree.)

- [ ] **Step 2: Guest src/lib.rs**

```rust
#![no_std]

// Generate guest bindings from the WIT. `path` is relative to the crate root;
// `../../wit` points at the repo `wit/` dir.
wit_bindgen::generate!({
    path: "../../wit/ruos-bringup.wit",
    world: "bringup",
});

// Minimal bump allocator so wit-bindgen's cabi_realloc has a backing allocator
// (the guest never frees; it runs once then the host tears the store down).
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
struct Bump { next: UnsafeCell<usize> }
unsafe impl Sync for Bump {}
#[global_allocator]
static A: Bump = Bump { next: UnsafeCell::new(0) };
const HEAP: usize = 1 << 20; // 1 MiB static arena
static mut ARENA: [u8; HEAP] = [0; HEAP];
unsafe impl GlobalAlloc for Bump {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let n = self.next.get();
        let base = ARENA.as_mut_ptr() as usize;
        let cur = (base + *n + l.align() - 1) & !(l.align() - 1);
        let end = cur + l.size();
        if end - base > HEAP { return core::ptr::null_mut(); }
        *n = end - base;
        cur as *mut u8
    }
    unsafe fn dealloc(&self, _: *mut u8, _: Layout) {}
}
#[panic_handler]
fn ph(_: &core::panic::PanicInfo) -> ! { loop {} }

struct Guest;
export!(Guest);

impl Guest for Guest {
    fn run() -> i32 {
        // Exercises a guest->host string import (the marker the boot test greps).
        crate::ruos::bringup::system::log("WT-COMPONENT-OK");
        0
    }
}
```
(The exact generated module path `ruos::bringup::system` and the `export!`/trait names come from `generate!`; if a build error names a different path, adjust to the generated symbols — `wit-bindgen` prints them. This is the one spot to reconcile against the installed `wit-bindgen` version.)

- [ ] **Step 3: Build the core module, then synthesize the component**

```bash
wsl -d Ubuntu -u root -e bash -lc 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem/tools/wt-bringup && TC=$(grep -hoE "nightly-[0-9]{4}-[0-9]{2}-[0-9]{2}" ../../rust-toolchain* 2>/dev/null | head -1); cargo +${TC:-nightly} build --release --target wasm32-wasip1 2>&1 | tail -15 && cd ../.. && wasm-tools component new tools/wt-bringup/target/wasm32-wasip1/release/wt_bringup.wasm -o build/wt-bringup.component.wasm 2>&1 | tail -5 && echo COMPONENT-BUILT'
```
Expected: `COMPONENT-BUILT`. The guest imports only `ruos:bringup/system` (no WASI) so `wasm-tools component new` needs **no** preview1 adapter. If it complains about unresolved `wasi_snapshot_preview1` imports, the guest accidentally pulled std — confirm `#![no_std]` and no std deps.

- [ ] **Step 4: Verify it is a valid component with the expected world**

```bash
wsl -d Ubuntu -u root -e bash -lc 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && wasm-tools component wit build/wt-bringup.component.wasm | grep -E "import ruos:bringup/system|export run" && echo WORLD-OK'
```
Expected: shows the `system` import + `run` export, then `WORLD-OK`.

- [ ] **Step 5: Commit**

```bash
cd /e/MinimalOS/BasicOperatingSystem
git add tools/wt-bringup/Cargo.toml tools/wt-bringup/src/lib.rs
git commit -m "feat(wasm): wt-bringup guest component (exports run, imports system.log)"
```

---

## Task 5: Teach wt-precompile to emit component `.cwasm`

**Files:**
- Modify: `tools/wt-precompile/src/main.rs`

- [ ] **Step 1: Add component mode**

In `tools/wt-precompile/src/main.rs`, after the existing arg parse and `Engine::new(&config)`, branch on a `--component` first arg. Replace the tail of `main` (the `precompile_module` + write) with:

```rust
    // Usage: wt-precompile [--component] <in.wasm> <out.cwasm>
    let (component_mode, in_path, out_path) = match args.as_slice() {
        [_, flag, i, o] if flag == "--component" => (true, i.clone(), o.clone()),
        [_, i, o] => (false, i.clone(), o.clone()),
        _ => { eprintln!("usage: wt-precompile [--component] <in.wasm> <out.cwasm>"); std::process::exit(2); }
    };
    let wasm = fs::read(&in_path).expect("read input wasm");
    // ... existing config setup is unchanged and already executed above ...
    let engine = Engine::new(&config).expect("create engine");
    let cwasm = if component_mode {
        engine.precompile_component(&wasm).expect("precompile component")
    } else {
        engine.precompile_module(&wasm).expect("precompile module")
    };
    fs::write(&out_path, &cwasm).expect("write output cwasm");
    eprintln!("wrote {} ({} bytes)", &out_path, cwasm.len());
```
Keep the existing `config` block (target, signals_based_traps(false), memory_* tunables, `x86_float_abi_ok`, `detect_host_feature`, the SSE4.1 cranelift flags) EXACTLY as-is — the component path must use the identical `Config` so its AOT settings hash matches the kernel engine.

- [ ] **Step 2: Build wt-precompile + produce the component cwasm**

```bash
wsl -d Ubuntu -u root -e bash -lc 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && (cd tools/wt-precompile && cargo build --release 2>&1 | tail -5) && tools/wt-precompile/target/release/wt-precompile --component build/wt-bringup.component.wasm kernel/src/wasm/wt/bringup.cwasm 2>&1 | tail -3 && echo CWASM-OK'
```
Expected: `wrote kernel/src/wasm/wt/bringup.cwasm (... bytes)` then `CWASM-OK`. (Placed next to the wt sources for `include_bytes!`, mirroring `hello.cwasm`/`gfxtest.cwasm`.)

- [ ] **Step 3: Commit**

```bash
cd /e/MinimalOS/BasicOperatingSystem
git add tools/wt-precompile/src/main.rs
git commit -m "feat(wt-precompile): --component mode (Engine::precompile_component)"
```

---

## Task 6: Kernel host side — bindgen, host impl, run_component

**Files:**
- Create: `kernel/src/wasm/wt/component.rs`
- Modify: `kernel/src/wasm/wt/mod.rs:5-9` (module list) + add the demo runner

- [ ] **Step 1: Write `kernel/src/wasm/wt/component.rs`**

```rust
//! Component Model bring-up: run a `ruos:bringup` component via wasmtime's
//! no_std component runtime (Component::deserialize + component::Linker), proving
//! the AOT component path works on bare metal. Mirrors run_cwasm's engine reuse.

use crate::kprintln;
use crate::wasm::wt::engine;
use wasmtime::component::{Component, Linker};
use wasmtime::Store;

// Generate host bindings from the SAME WIT the guest used. `path` is relative to
// the crate root (kernel/). The macro emits a `Bringup` instance type + a
// `system::Host` trait we implement on the store data.
wasmtime::component::bindgen!({
    path: "../wit/ruos-bringup.wit",
    world: "bringup",
});

/// Store data for bring-up: implements the generated `system` host trait.
struct BringupHost;

impl ruos::bringup::system::Host for BringupHost {
    fn log(&mut self, msg: alloc::string::String) {
        kprintln!("[component] {}", msg);
    }
    fn poweroff(&mut self) {
        crate::power::poweroff();
    }
}

/// Deserialize + instantiate + call `run` on a precompiled bring-up component.
/// Returns the guest's run() result, or a negative code on host-side failure.
pub fn run_component(cwasm: &[u8]) -> i32 {
    let engine = engine();
    // SAFETY: produced by wt-precompile --component for this exact engine Config.
    let component = match unsafe { Component::deserialize(engine, cwasm) } {
        Ok(c) => c,
        Err(e) => { kprintln!("ruos: component deserialize err: {:?}", e); return -1; }
    };
    let mut store = Store::new(engine, BringupHost);
    let mut linker: Linker<BringupHost> = Linker::new(engine);
    // Generated: wires the `system` import to BringupHost's Host impl.
    if let Err(e) = Bringup::add_to_linker(&mut linker, |s: &mut BringupHost| s) {
        kprintln!("ruos: component link err: {:?}", e); return -2;
    }
    // Match run_cwasm: SysV ABI requires DF=0; firmware may leave it set.
    #[cfg(target_arch = "x86_64")]
    unsafe { core::arch::asm!("cld", options(nostack)); }
    let bringup = match Bringup::instantiate(&mut store, &component, &linker) {
        Ok(b) => b,
        Err(e) => { kprintln!("ruos: component instantiate err: {:?}", e); return -3; }
    };
    match bringup.call_run(&mut store) {
        Ok(code) => code,
        Err(e) => { kprintln!("ruos: component run err: {:?}", e); -4 }
    }
}
```
NOTE: the generated names (`Bringup`, `Bringup::add_to_linker`, `Bringup::instantiate`, `bringup.call_run`, `ruos::bringup::system::Host`) follow wasmtime 45's `bindgen!` conventions (world `bringup` → `Bringup`; export `run` → `call_run`). If a compile error reports different generated symbols, adjust to them — `cargo build` names them exactly. `bindgen!` defaults to SYNC bindings (no `async`), which is what we want. Add `use alloc::string::String;` if the macro expands `String` unqualified.

- [ ] **Step 2: Wire the module + boot demo in `kernel/src/wasm/wt/mod.rs`**

Add to the module list (near `pub mod gfx;`):
```rust
pub mod component;
```
Add the embedded component + demo runner (mirror `run_hello_demo`, behind `boot-checks`):
```rust
#[cfg(feature = "boot-checks")]
static BRINGUP_CWASM: &[u8] = include_bytes!("bringup.cwasm");

/// Boot self-test: run the embedded bring-up component; its `run` calls
/// system.log("WT-COMPONENT-OK") on the host. Returns the guest run() code.
#[cfg(feature = "boot-checks")]
pub fn run_bringup_demo() -> i32 {
    crate::wasm::wt::component::run_component(BRINGUP_CWASM)
}
```

- [ ] **Step 3: Build the kernel (boot-checks) to typecheck the bindgen + host**

```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && source $HOME/.cargo/env && cd kernel && cargo build --release -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem --target x86_64-unknown-none --features boot-checks 2>&1 | tail -25'
```
Expected: `Finished`. Compile errors here are almost all generated-name mismatches (Step 1 NOTE) — read the error, adjust the symbol, rebuild. Do not proceed until it builds.

- [ ] **Step 4: Commit**

```bash
cd /e/MinimalOS/BasicOperatingSystem
git add kernel/src/wasm/wt/component.rs kernel/src/wasm/wt/mod.rs kernel/src/wasm/wt/bringup.cwasm
git commit -m "feat(wasm): run_component + ruos:bringup host bindings (bindgen!)"
```

---

## Task 7: Boot self-test wiring + Makefile build integration

**Files:**
- Modify: `kernel/src/boot/phases/interrupts.rs:54-72` (boot-checks block)
- Modify: `Makefile` (build the component + precompile before the kernel)

- [ ] **Step 1: Emit the marker from the boot-checks block**

In `kernel/src/boot/phases/interrupts.rs`, inside the existing `#[cfg(feature = "boot-checks")]` block (after the wasmtime hello line), add:

```rust
        // Component Model bring-up: prove the no_std AOT component path runs.
        let cc = crate::wasm::wt::run_bringup_demo();
        crate::binfo!("wt", "component bringup run={}", cc);
```
The guest's `run` logs `[component] WT-COMPONENT-OK` to serial; the boot test greps that string.

- [ ] **Step 2: Makefile — produce build/bringup.cwasm and the embedded copy**

Add near the `build/gui.cwasm` rule (after Task 1's `RUOS_DESKTOP_SRCS` block). Use the host tool + wasm-tools:
```makefile
# Bring-up component (Step-0 gate): guest -> component -> AOT cwasm embedded in kernel.
kernel/src/wasm/wt/bringup.cwasm: wit/ruos-bringup.wit tools/wt-bringup/src/lib.rs tools/wt-bringup/Cargo.toml $(WT_PRECOMPILE)
	@mkdir -p build
	source $$HOME/.cargo/env && cd tools/wt-bringup && \
		cargo build --release --target wasm32-wasip1
	source $$HOME/.cargo/env && wasm-tools component new \
		tools/wt-bringup/target/wasm32-wasip1/release/wt_bringup.wasm \
		-o build/wt-bringup.component.wasm
	$(WT_PRECOMPILE) --component build/wt-bringup.component.wasm kernel/src/wasm/wt/bringup.cwasm
```
Make the boot-checks kernel build depend on it: in the `test-boot` target prerequisites (`Makefile:349`), add `kernel/src/wasm/wt/bringup.cwasm` to the list.

- [ ] **Step 3: Run the headless boot test (fresh ISO path, don't touch os.iso)**

```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make test-boot ISO=build/cmtest.iso 2>&1 | tail -15'
```
Expected: `TEST_BOOT_PASS`.

- [ ] **Step 4: Confirm the component actually ran (the decisive proof)**

```bash
cd /e/MinimalOS/BasicOperatingSystem && grep -E "component bringup run=0|\[component\] WT-COMPONENT-OK" build/test-boot.log
```
Expected: BOTH lines present — `[component] WT-COMPONENT-OK` (guest→host import fired) and `wt component bringup run=0` (guest export returned 0). This proves `Component::deserialize` + `component::Linker` + instantiate + host-call + guest-export all work in the no_std AOT runtime on bare metal.

- [ ] **Step 5: Confirm no regression in the existing self-tests**

```bash
cd /e/MinimalOS/BasicOperatingSystem && grep -E "zero-init self-test ok|gfx blit self-test ok|exec W\^X self-test ok" build/test-boot.log
```
Expected: all three still `ok` (component-model feature did not perturb the core-module path).

- [ ] **Step 6: Commit**

```bash
cd /e/MinimalOS/BasicOperatingSystem
git add kernel/src/boot/phases/interrupts.rs Makefile
git commit -m "test(wasm): boot self-test runs the ruos:bringup component (Step-0 gate)"
```

---

## Task 8: De-risk verify on VBox + record the gate result

**Files:**
- Create: `CHANGELOG/273-26-06-04-wasm-component-bringup.md`

- [ ] **Step 1: Boot the bring-up ISO on VBox (or QEMU with display) and read serial**

The bring-up runs at boot under boot-checks; the marker is on serial. If using the existing VBox VM, build a boot-checks ISO to a scratch path and point the VM's CD at it temporarily, OR run QEMU with serial:
```bash
wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 60 qemu-system-x86_64 -machine q35 -cpu max -m 512 -no-reboot -display none -serial stdio -cdrom build/cmtest.iso 2>&1 | grep -E "WT-COMPONENT-OK|component bringup" '
```
Expected: the marker prints. (VBox-specific run only needed if a CPU/MSR-sensitive difference is suspected — per project memory, verify on VBox for CPU/MSR/STI-sensitive changes; the component runtime is pure userspace codegen, so QEMU is normally sufficient for this gate.)

- [ ] **Step 2: Write the changelog**

```markdown
# 273 — WASM Component Model bring-up (Step 0 gate) PASSES

**Data:** 2026-06-04

## Cosa
Provato end-to-end che il Component Model di wasmtime gira nel runtime no_std AOT
di ruos: un component `ruos:bringup` (import `system.log`/`poweroff`, export `run`)
viene deserializzato + istanziato + eseguito al boot (boot-check), e la sua
`run` chiama `system.log("WT-COMPONENT-OK")` sull'host.

## Perché
Gate decisivo del piano WIT/Component-Model (spec 2026-06-04): "compila" era già
provato; questo prova "gira" su bare metal, sbloccando la migrazione del desktop.

## Come
wit/ruos-bringup.wit (sorgente unica) -> guest tools/wt-bringup (wit-bindgen) ->
wasm-tools component new -> wt-precompile --component (Engine::precompile_component,
stessa Config del kernel) -> kernel run_component (Component::deserialize +
component::Linker + bindgen! host impl). Feature wasmtime component-model abilitata
(no_std confermato). run_cwasm e i tool wasip1 invariati.

## File toccati
- wit/ruos-bringup.wit, tools/wt-bringup/*, tools/wt-precompile/src/main.rs
- kernel/Cargo.toml, kernel/src/wasm/wt/component.rs, kernel/src/wasm/wt/mod.rs
- kernel/src/boot/phases/interrupts.rs, Makefile
```

- [ ] **Step 3: Commit**

```bash
cd /e/MinimalOS/BasicOperatingSystem
git add CHANGELOG/273-26-06-04-wasm-component-bringup.md
git commit -m "docs(changelog): 273 — component-model bring-up gate passes"
```

---

## Follow-on plans (NOT in this plan)

Once this gate is green, subsequent plans implement the spec's later steps:
- **Plan 2 — egui through the component:** port `ruos-backend` to a component + author `ruos:gui/{gfx,input,clock}` + `ruos:system/power` WIT (surface-resource shape, single fullscreen surface) + switch the GUI launch to `run_component`; re-verify egui text (garble), cursor, clock, zero-init through the component codegen. gui-core's `Platform` seam becomes the generated import trait; pc-backend implements it.
- **Plan 3 — poweroff button:** wire `system.poweroff` to a desktop UI button as the first real capability on the new layer.
- **Plan 4 (optional, later):** fold WASI fs/stdio into `wasi:filesystem`/`wasi:io` with resource handles.

## Self-Review notes

- **Spec coverage:** This plan implements spec §2 (decision A), §3 (kernel host via bindgen + Component::deserialize + component::Linker; wt-precompile --component), §5 Step 0 (bring-up gate) and the `system/power` import shape; §5 Steps 1-5 + §4 surface-resources are explicitly deferred to Plans 2-3. §6 "what stays unchanged" is honored (run_cwasm + wasip1 tools + engine_config untouched).
- **Placeholders:** none — every step has concrete code/commands. The two reconciliation points (generated wit-bindgen guest symbol path in Task 4 Step 2; generated `bindgen!` host symbol names in Task 6 Step 1) are explicit "build-and-adjust-to-the-compiler" gates, not vague TODOs — unavoidable because exact generated identifiers are toolchain-version-specific.
- **Type/name consistency:** the marker string `WT-COMPONENT-OK` and the `run`/`call_run` + `system.log` names are used consistently across guest (Task 4), host (Task 6), and the boot grep (Task 7).
