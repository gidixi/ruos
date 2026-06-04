# Wasmtime no_std AOT Spike Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove that an AOT-precompiled `.cwasm` module runs inside ruos under a
`no_std` Wasmtime runtime (no Cranelift on-device), calling one host function
that prints to serial — the decision gate for the whole egui-desktop effort.

**Architecture:** The build host (WSL) compiles a trivial wasm module to a
target-specific `.cwasm` with the Wasmtime CLI (Cranelift runs there). ruos links
the `wasmtime` crate with `default-features = false` (no compiler), implements the
required platform-shim symbols (memory via the kernel heap, executable memory via
`crate::memory::exec` from plan #2, abort/trap), deserialises the `.cwasm`,
instantiates it with one imported host function, and runs it.

**Tech Stack:** Rust `no_std`, `wasmtime` crate (runtime-only, AOT),
`crate::memory::exec` (W^X, plan #2), Limine modules, Wasmtime CLI on the host.

**⚠️ THIS IS A SPIKE.** Unlike plans #1/#2, parts of this involve genuine API
discovery: the exact `wasmtime` feature set, the precise platform-shim symbol
list, and `no_std` dependency breakage are not fully knowable in advance. Steps
marked **(INVESTIGATE)** require reading current Wasmtime docs/source and
recording findings before coding. The plan ends in an explicit **GO / NO-GO**
decision.

**Authoritative references** (read before starting):
- Platform support: https://docs.wasmtime.dev/stability-platform-support.html
- Minimal embedding: https://docs.wasmtime.dev/examples-minimal.html
- `examples/min-platform/` in the wasmtime repo (the `wasmtime-platform.h` symbol set)
- no_std tracking issue: https://github.com/bytecodealliance/wasmtime/issues/8341
- Theseus port writeup: https://www.theseus-os.com/2022/06/21/wasmtime-complete-no_std-port.html

**Depends on:** plan #2 (`crate::memory::exec`). Plan #1 is independent.

**Build/run via WSL** (per CLAUDE.md): wrap `make`/`cargo`/`wasmtime` in
`wsl -d Ubuntu -u root -e bash -c 'cd <repo-on-wsl> && <cmd>'`.

---

## File Structure

- Modify: `kernel/Cargo.toml` — add pinned `wasmtime` dependency (no_std).
- Create: `kernel/src/wasm/wt/mod.rs` — Wasmtime runtime wrapper (engine, load, run).
- Create: `kernel/src/wasm/wt/platform.rs` — platform-shim symbols.
- Modify: `kernel/src/wasm/mod.rs` — declare `pub mod wt;`.
- Modify: `kernel/src/boot/phases/userland.rs` (or wherever demos launch) — boot-check that runs the hello `.cwasm`.
- Create: `tools/wt-hello/` (host crate or `.wat`) — the trivial guest.
- Modify: `Makefile` — host step to build `hello.cwasm` + stage it as a Limine module.
- Modify: `limine.conf` — add the `hello.cwasm` module (test build only).

---

## Task 1: Pick and pin the Wasmtime version (INVESTIGATE)

**Files:** none yet (research + notes)

- [ ] **Step 1: Determine the latest `wasmtime` release that supports `no_std` runtime-only AOT**

Run on the host:
`wsl -d Ubuntu -u root -e bash -c 'cargo search wasmtime | head -3'`
Open the Platform Support doc and confirm the chosen version documents the
no_std / "custom platform" / AOT path. Record the exact version `X.Y.Z`.

- [ ] **Step 2: Determine the exact feature set**

(INVESTIGATE) From the minimal-embedding docs and the crate's `Cargo.toml`
features, identify the feature combination for: runtime ON, Cranelift OFF, Winch
OFF, `std` OFF, signals-based-traps OFF. The likely shape is:
```toml
wasmtime = { version = "=X.Y.Z", default-features = false, features = ["runtime"] }
```
but confirm against the chosen version (feature names drift between releases).
Record the final feature list and any required companion crate
(`wasmtime-environ`, etc.).

- [ ] **Step 3: Install the matching Wasmtime CLI on the host**

The CLI that produces `.cwasm` MUST match the crate version exactly (cwasm is
version-locked). Run:
`wsl -d Ubuntu -u root -e bash -c 'cargo install wasmtime-cli --version =X.Y.Z'`
Verify: `wsl ... 'wasmtime --version'` prints `X.Y.Z`.

- [ ] **Step 4: Record findings**

Append a short note block to THIS file under a new `## Findings` heading: chosen
version, feature list, CLI install confirmed. (No commit — notes only, committed
with Task 2.)

---

## Task 2: Add the dependency and get it to compile (INVESTIGATE / hardest step)

**Files:**
- Modify: `kernel/Cargo.toml`

- [ ] **Step 1: Add the dependency**

In `kernel/Cargo.toml` `[dependencies]`, add (using the version/features from
Task 1):

```toml
wasmtime = { version = "=X.Y.Z", default-features = false, features = ["runtime"] }
```

- [ ] **Step 2: Attempt the build and triage breakage**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd <repo-on-wsl> && cargo build -p kernel --target x86_64-unknown-none 2>&1 | tee build/wt-build.log'`
(Use the project's actual target/flags — mirror what `make iso` invokes; check
the Makefile/`.cargo/config.toml` for the exact `--target` and `-Z build-std`.)

Expected: this WILL surface errors. Triage each:
- **Missing platform symbols** (link/`extern` errors like `wasmtime_*`): these
  are handled in Task 3 — note them, continue.
- **A dependency pulling `std`** (e.g. a transitive crate without no_std): record
  which crate. Mitigations, in order: enable that crate's no_std feature via a
  `[patch]`/feature unification; pin the crate version (Theseus needed
  `bincode 2.0`); if truly blocked, this is a NO-GO signal — record it.
- **`getrandom`/time**: ruos already wires a custom `getrandom` backend
  (`ssh/rng_bridge.rs`) — reuse that pattern if Wasmtime pulls `getrandom`.

- [ ] **Step 3: Iterate until the only remaining errors are missing platform symbols**

Resolve std-pulling deps until `cargo build` fails ONLY on the platform-shim
symbols (or succeeds). Record the final dependency tweaks. If a dep cannot be
made no_std after reasonable effort, STOP and jump to Task 6 (NO-GO).

- [ ] **Step 4: Commit the dependency state**

```bash
git add kernel/Cargo.toml Cargo.lock docs/superpowers/plans/2026-06-04-wasmtime-nostd-spike.md
git commit -m "build(wasm): add pinned no_std wasmtime dependency (spike)"
```

---

## Task 3: Implement the platform shim (INVESTIGATE + implement)

**Files:**
- Create: `kernel/src/wasm/wt/platform.rs`
- Modify: `kernel/src/wasm/mod.rs`

- [ ] **Step 1: Enumerate the required symbols (INVESTIGATE)**

From `examples/min-platform/embedding/wasmtime-platform.h` (the version matching
Task 1), list every symbol Wasmtime expects when `std` is off and
signals-based-traps is off. Typical set (CONFIRM against your version):
- virtual/heap memory: `wasmtime_mmap_new`, `wasmtime_mmap_remap`,
  `wasmtime_munmap`, `wasmtime_mprotect` (or the alloc-based equivalents when
  `custom-virtual-memory` is off)
- executable memory: the path Wasmtime uses to make code RX
- `wasmtime_setjmp` / `wasmtime_longjmp` (trap unwinding without signals)
- `wasmtime_tls_get` / `wasmtime_tls_set`
- abort/`wasmtime_*` panic hooks

Record the EXACT list for your version. Do NOT trust this plan's list verbatim —
it varies by release.

- [ ] **Step 2: Declare the module**

In `kernel/src/wasm/mod.rs`, add to the module list (near line 4-10):

```rust
pub mod wt;
```

And create `kernel/src/wasm/wt/mod.rs` with at least:

```rust
//! Wasmtime no_std AOT runtime (spike). Runs precompiled `.cwasm` modules.
pub mod platform;
```

- [ ] **Step 3: Implement the shim**

Create `kernel/src/wasm/wt/platform.rs`. Map each required symbol to a ruos
primitive. Skeleton (fill per Task 3 Step 1 findings — names/signatures MUST
match the header for your version):

```rust
//! Platform-shim symbols required by no_std Wasmtime. Memory comes from the
//! kernel heap; executable memory from `crate::memory::exec` (W^X, plan #2);
//! traps via a setjmp/longjmp pair; no OS signals.

use core::ffi::c_void;

// --- Executable / virtual memory -----------------------------------------
// EXACT signatures depend on the wasmtime-platform.h for the pinned version.
// The intent: allocate a region, allow Wasmtime to write code, then make it
// executable. Back code regions with crate::memory::exec; back data/linear
// memory with the global allocator.

#[no_mangle]
pub extern "C" fn wasmtime_mmap_new(/* size, prot, ret */) -> i32 {
    // Allocate via crate::memory::exec::alloc_exec for executable requests,
    // or a heap-backed region for data. Return 0 on success.
    todo!("implement per platform header for pinned version")
}

#[no_mangle]
pub extern "C" fn wasmtime_mprotect(/* ptr, len, prot */) -> i32 {
    // For PROT_EXEC: crate::memory::exec::protect_exec on the region.
    todo!("implement per platform header")
}

#[no_mangle]
pub extern "C" fn wasmtime_munmap(/* ptr, len */) -> i32 {
    todo!("implement per platform header")
}

// --- Trap unwinding (no signals) -----------------------------------------
#[no_mangle]
pub extern "C" fn wasmtime_setjmp(/* ... */) -> i32 { todo!() }
#[no_mangle]
pub extern "C" fn wasmtime_longjmp(/* ... */) -> ! { todo!() }

// --- TLS ------------------------------------------------------------------
static mut WT_TLS: *mut c_void = core::ptr::null_mut();
#[no_mangle]
pub extern "C" fn wasmtime_tls_get() -> *mut c_void { unsafe { WT_TLS } }
#[no_mangle]
pub extern "C" fn wasmtime_tls_set(p: *mut c_void) { unsafe { WT_TLS = p; } }
```

> The `todo!()`s are placeholders ONLY because the exact ABI is version-specific
> and must be read from the header in Step 1. Replace EVERY `todo!()` with a real
> implementation before building — a `todo!()` reaching runtime is a spike
> failure. (Per the no-placeholder rule: this is the one task where the concrete
> code is gated on a documented investigation; do the investigation, then write
> the real code.)

- [ ] **Step 4: Build until the platform symbols resolve**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd <repo-on-wsl> && make iso'`
Expected: link succeeds (all `wasmtime_*` symbols defined). Trap setjmp/longjmp
may be the trickiest — if x86-64 setjmp/longjmp in Rust is hard, implement the
minimal register save/restore in inline asm (callee-saved regs + rsp + return
address), or check whether the version offers a "no unwinding" trap mode.

- [ ] **Step 5: Commit**

```bash
git add kernel/src/wasm/wt/ kernel/src/wasm/mod.rs
git commit -m "feat(wasm): wasmtime no_std platform shim (memory/trap/tls)"
```

---

## Task 4: The hello guest + host AOT build

**Files:**
- Create: `tools/wt-hello/hello.wat`
- Modify: `Makefile`
- Modify: `limine.conf`

- [ ] **Step 1: Write the trivial guest**

Create `tools/wt-hello/hello.wat` — imports one host fn `print(i32)` and calls it
with 42, then returns:

```wat
(module
  (import "ruos" "print" (func $print (param i32)))
  (func (export "run")
    i32.const 42
    call $print))
```

- [ ] **Step 2: Add the host build step**

In the `Makefile`, add a target that compiles the wat to wasm then AOT to cwasm
(requires `wabt`'s `wat2wasm` and the pinned `wasmtime` CLI):

```make
build/hello.cwasm: tools/wt-hello/hello.wat
	wat2wasm tools/wt-hello/hello.wat -o build/hello.wasm
	wasmtime compile --target x86_64-unknown-none build/hello.wasm -o build/hello.cwasm
```

(Confirm the correct `--target` triple for ruos against
`wasmtime targets`/docs; `x86_64-unknown-none` is the expected bare-metal triple
but verify for your version.) Wire `build/hello.cwasm` as a prerequisite of the
test ISO target so it is staged as a Limine module.

- [ ] **Step 3: Stage it as a Limine module**

In `limine.conf` (test build), add a module entry pointing at `hello.cwasm` so it
lands in the VFS (mirror how existing `.wasm` modules are listed). Confirm the
path the kernel will read (e.g. `/bin/hello.cwasm`).

- [ ] **Step 4: Build and confirm the module is present**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd <repo-on-wsl> && make iso CARGO_FEATURES=boot-checks'`
Expected: `build/hello.cwasm` exists and the ISO build does not error on the
module entry.

- [ ] **Step 5: Commit**

```bash
git add tools/wt-hello/hello.wat Makefile limine.conf
git commit -m "build(wasm): host AOT step for hello.cwasm + limine module"
```

---

## Task 5: Load, link host fn, run, assert

**Files:**
- Modify: `kernel/src/wasm/wt/mod.rs`
- Modify: `kernel/src/boot/phases/userland.rs` (boot-check launch point)

- [ ] **Step 1: Implement load+run in the wt wrapper**

In `kernel/src/wasm/wt/mod.rs`, add (adapt API names to the pinned wasmtime
version — `Engine`, `Module::deserialize`, `Linker`/`Instance`, `Store`):

```rust
use crate::kprintln;

/// Run a precompiled `.cwasm` whose `run` export calls imported `ruos.print`.
/// Returns true if the guest invoked `print(42)`. (Spike: minimal error paths.)
pub fn run_hello(cwasm: &[u8]) -> bool {
    use wasmtime::{Engine, Module, Store, Linker, Config};

    let mut config = Config::new();
    // Ensure compilation is OFF (AOT only) and any no_std-required toggles set.
    // (INVESTIGATE: the exact Config calls for your version.)
    let engine = match Engine::new(&config) { Ok(e) => e, Err(_) => return false };

    // SAFETY: cwasm was produced by the matching wasmtime version for this target.
    let module = match unsafe { Module::deserialize(&engine, cwasm) } {
        Ok(m) => m,
        Err(_) => return false,
    };

    let mut store = Store::new(&engine, false); // store data = "saw 42" flag
    let mut linker = Linker::new(&engine);
    linker.func_wrap("ruos", "print", |mut caller: wasmtime::Caller<'_, bool>, v: i32| {
        kprintln!("ruos: wt hello print={}", v);
        if v == 42 { *caller.data_mut() = true; }
    }).ok();

    let instance = match linker.instantiate(&mut store, &module) {
        Ok(i) => i,
        Err(_) => return false,
    };
    let run = match instance.get_typed_func::<(), ()>(&mut store, "run") {
        Ok(f) => f,
        Err(_) => return false,
    };
    let _ = run.call(&mut store, ());
    *store.data()
}
```

- [ ] **Step 2: Launch it from a boot-check**

In `kernel/src/boot/phases/userland.rs`, in a `#[cfg(feature = "boot-checks")]`
block (after the VFS is up so the module is readable), add:

```rust
    #[cfg(feature = "boot-checks")]
    {
        if let Ok(bytes) = crate::wasm::read_all("/bin/hello.cwasm").await {
            let ok = crate::wasm::wt::run_hello(&bytes);
            crate::binfo!("wt", "wasmtime AOT hello {}", if ok { "ok" } else { "FAIL" });
        } else {
            crate::binfo!("wt", "wasmtime AOT hello FAIL (module missing)");
        }
    }
```

(Confirm `read_all` is reachable/`async` here — it is `pub(crate) async` in
`wasm/mod.rs`. Place the call where async is allowed in this phase; if the phase
is sync, run it via the executor like other boot tasks.)

- [ ] **Step 3: Run the gate test**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd <repo-on-wsl> && make test-boot'`
Expected (GO): log shows `ruos: wt hello print=42` and `wt: wasmtime AOT hello ok`.

- [ ] **Step 4: Commit**

```bash
git add kernel/src/wasm/wt/mod.rs kernel/src/boot/phases/userland.rs
git commit -m "feat(wasm): run hello.cwasm under no_std wasmtime AOT (spike gate)"
```

---

## Task 6: GO / NO-GO decision + changelog

**Files:**
- Modify: this plan (`## Decision` section)
- Create: `CHANGELOG/NN-26-06-04-wasmtime-nostd-spike.md`

- [ ] **Step 1: Record the outcome**

Add a `## Decision` section to this file:
- **GO** if `wt: wasmtime AOT hello ok` appears: proceed to plans #4-#7
  (`ruos_gfx`, gui app, terminal, router). Note kernel size delta
  (`ls -la build/*.iso` before/after) and any perf caveats observed.
- **NO-GO** if a dependency could not be made no_std, the shim could not be
  completed, or the module would not run: record the specific blocker and switch
  the desktop effort to the **fallback** (wasmi interpreter + §9 mitigations, per
  spec §11). Plans #4-#7 then target wasmi instead of Wasmtime.

- [ ] **Step 2: Write the changelog entry**

```markdown
# NN — Spike: Wasmtime no_std AOT

**Data:** 2026-06-04

## Cosa
Spike di integrazione Wasmtime no_std in modalità AOT: dipendenza pinnata
(runtime-only, no Cranelift), platform shim (memoria/exec via memory::exec,
trap setjmp/longjmp, TLS), build host `.cwasm`, esecuzione di hello.cwasm che
chiama host fn `ruos.print(42)`. Esito GO/NO-GO registrato nel piano.

## Perché
Gate decisionale del desktop egui: l'AOT dà velocità quasi-nativa senza portare
Cranelift nel kernel. Se NO-GO → fallback wasmi (spec §11).

## File toccati
- kernel/Cargo.toml, kernel/src/wasm/wt/{mod,platform}.rs, kernel/src/wasm/mod.rs
- kernel/src/boot/phases/userland.rs
- tools/wt-hello/hello.wat, Makefile, limine.conf
```

- [ ] **Step 3: Commit**

```bash
git add docs/superpowers/plans/2026-06-04-wasmtime-nostd-spike.md CHANGELOG/NN-26-06-04-wasmtime-nostd-spike.md
git commit -m "docs: wasmtime spike decision + changelog"
```

---

## Decision: **GO** (verified 2026-06-04)

The spike succeeded end-to-end in QEMU (`-cpu max`):

```
INFO mem  exec W^X self-test ok
ruos: wt hello print=42
INFO wt   wasmtime AOT hello ok
shell: init.sh complete            ← full boot, no regression
```

A host-precompiled `hello.cwasm` runs under no_std Wasmtime 45 in ruos: it
deserialises, places native code into W^X exec pages via the VM shim, instantiates,
links host fn `ruos.print`, and the guest calls `print(42)`. Proceed to plans
#4–#7 (Wasmtime AOT for the GUI).

### Actual recipe (differs from the pre-spike guesses above)

- **Version:** `wasmtime = "=45.0.0"`, kernel features
  `["runtime", "custom-virtual-memory"]` (default-features off). Host precompiler
  `tools/wt-precompile` uses `["runtime", "cranelift"]`.
- **Platform shim symbols actually required** (in `kernel/src/wasm/wt/platform.rs`):
  `wasmtime_tls_get/set`, and for `custom-virtual-memory`:
  `wasmtime_page_size`, `wasmtime_mmap_new`, `wasmtime_mmap_remap`,
  `wasmtime_munmap`, `wasmtime_mprotect`, `wasmtime_memory_image_new/map_at/free`
  (images declined → `*ret = NULL`). **No `setjmp`/`longjmp`, no signal symbols**
  (`signals-based-traps` off). mmap/mprotect are backed by the frame allocator +
  paging (a dedicated VA window); this supersedes the standalone `memory::exec`
  for Wasmtime's needs (exec W^X plan #2 still validates the executable-page
  mechanism).
- **The hard part was cwasm↔runtime settings compatibility.** Both sides must use
  an IDENTICAL `Config`: `signals_based_traps(false)`, `memory_init_cow(false)`,
  `memory_reservation(0)`, `memory_guard_size(0)`,
  `memory_reservation_for_growth(0)`, `memory_may_move(true)`,
  `x86_float_abi_ok(true)`, and a **fixed** `detect_host_feature` policy
  (`sse3/ssse3/sse4.1/sse4.2 = true`, everything else false) so the host doesn't
  bake its native AVX/AVX-512 ISA into the module. The host precompiler also
  calls `config.target("x86_64-unknown-none")`; the kernel must NOT (it already
  IS that target).
- **Kernel size:** ~20.5 MB → ~32.5 MB (release, debuginfo on; strippable).

### Follow-ups for the production plan

- The fixed ISA policy assumes SSE4.2 (universal on x86_64 since ~2008; present
  under `-cpu max`). Keep it fixed for determinism, or detect at runtime AND pin
  the precompiler to the same set.
- Replace `include_bytes!`-of-cwasm with a Makefile step (`wt-precompile` →
  `/bin/*.cwasm` Limine module) + load via VFS; add the runtime router
  (`.cwasm` → Wasmtime).
- Integrate the cooperative blocking model (epoch yield) for real WASI/`ruos_gfx`
  host calls — the hello demo is synchronous.

## Self-Review notes (already applied)

- **Spec coverage:** implements §5 (Wasmtime platform shim, signals-off),
  §3 (AOT build pipeline host→`.cwasm`→Limine module), and §13 prereq #3 (the
  gate). The GO/NO-GO step maps directly to the §11/§12 fallback strategy.
- **Honesty about placeholders:** the `todo!()`s in Task 3 are explicitly gated
  on the version-specific header read in Task 3 Step 1 and MUST be replaced with
  real code before building. This is a deliberate spike structure, not a hidden
  TODO — the task spells out the investigation that produces the code.
- **Risk front-loaded:** the two highest-risk items (no_std dep breakage Task 2;
  trap/setjmp shim Task 3) come before any GUI work, so a NO-GO costs the least.
- **Version locking:** the cwasm-vs-runtime version match is enforced by pinning
  both the crate (`=X.Y.Z`) and the CLI (`--version =X.Y.Z`) in Task 1.
```
