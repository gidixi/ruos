# Executable Memory (W^X) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an executable-memory allocator (write-then-protect, W^X) on top of the existing paging API, so a later Wasmtime AOT runtime can load native code into RX pages.

**Architecture:** A dedicated higher-half virtual window is bump-allocated 4 KiB
at a time. `alloc_exec` maps fresh physical frames into that window as
writable + non-executable (for code emission); `protect_exec` flips them to
read-only + executable (clearing WRITABLE, clearing NO_EXECUTE) — so no virtual
address is ever both writable and executable. A new `mapper::set_flags` wraps the
`x86_64` crate's `update_flags`. EFER.NXE is already enabled (the kernel already
uses `NO_EXECUTE` in `map_io_page`), so the NX bit is honoured.

**Tech Stack:** Rust `no_std`, `x86_64` paging (`Mapper::update_flags`,
`PageTableFlags`), existing `crate::memory` (`map_page`, `unmap_page`,
`allocate_frame`, `free_frame`).

**Testing model:** ruos boot-check self-test (see plan #1 header). The W^X
self-test writes real machine code (`mov eax,42; ret`) into an exec allocation,
protects it, calls it through a function pointer, and asserts it returns 42 — an
end-to-end proof that executable memory works.

**This plan is #2 of the series** (spec §13). Depends on nothing from plan #1;
both are independent prerequisites. Plan #3 (Wasmtime spike) consumes
`crate::memory::exec`.

**Build/run via WSL** (per CLAUDE.md): wrap each `make` in
`wsl -d Ubuntu -u root -e bash -c 'cd <repo-on-wsl> && <cmd>'`
(`<repo-on-wsl>` e.g. `/mnt/w/Work/GitHub/ruos`).

---

## File Structure

- Modify: `kernel/src/memory/mapper.rs` — add `set_flags`.
- Create: `kernel/src/memory/exec.rs` — exec window allocator + `self_test`.
- Modify: `kernel/src/memory/mod.rs` — declare + re-export `exec`.
- Modify: `kernel/src/boot/phases/interrupts.rs` — boot-check self-test call.

---

## Task 1: `mapper::set_flags` (change flags on a mapped page)

**Files:**
- Modify: `kernel/src/memory/mapper.rs`

- [ ] **Step 1: Verify the `allocate_frame` / `free_frame` signatures**

`exec.rs` (Task 2) needs the exact types. Run:
`grep -n "pub fn allocate_frame\|pub fn free_frame" kernel/src/memory/frames.rs`
Record the signatures. This plan assumes `allocate_frame() -> Option<u64>`
(physical address) and `free_frame(phys: u64)`. If they instead use
`PhysFrame`/`PhysAddr`, adapt the conversions in Task 2 accordingly (the plan
notes where).

- [ ] **Step 2: Add `set_flags`**

In `kernel/src/memory/mapper.rs`, add the import near the top (with the other
`mapper::` import on line 11):

```rust
use x86_64::structures::paging::mapper::FlagUpdateError;
```

Then add, after `unmap_page` (after line 112):

```rust
/// Change the flags of an already-mapped 4 KiB page (and flush its TLB entry).
/// Used to flip executable-memory pages from W (writable, NX) to X (read-only,
/// executable) — the W^X protection step.
pub fn set_flags(virt: VirtAddr, flags: PageTableFlags) -> Result<(), UnmapError> {
    let mut g_map = MAPPER.lock();
    let mapper = g_map.as_mut().ok_or(UnmapError::NotInitialized)?;
    let page: Page<Size4KiB> = Page::containing_address(virt);
    // SAFETY: caller guarantees `flags` is a valid combination for an existing
    // mapping; update_flags does not change the frame, only its permissions.
    unsafe {
        mapper.update_flags(page, flags)
            .map_err(|e| match e {
                FlagUpdateError::PageNotMapped       => UnmapError::NotMapped,
                FlagUpdateError::ParentEntryHugePage => UnmapError::ParentHugePage,
            })?
            .flush();
    }
    Ok(())
}
```

- [ ] **Step 3: Add the export**

In `kernel/src/memory/mod.rs`, add `set_flags` to the `mapper::` re-export
(line 13-14):

```rust
pub use mapper::{MapError, UnmapError, init as init_mapper, map_page, unmap_page, map_io_page,
    map_io_range, hhdm_virt, hhdm_offset, set_flags};
```

- [ ] **Step 4: Run to verify it builds**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd <repo-on-wsl> && make iso'`
Expected: clean build. (No behaviour change yet — exercised in Task 3.)
If `update_flags` reports a different `FlagUpdateError` variant set for the
`x86_64 = 0.15` crate, the compiler will name them; make the match exhaustive
over what it lists.

- [ ] **Step 5: Commit**

```bash
git add kernel/src/memory/mapper.rs kernel/src/memory/mod.rs
git commit -m "feat(memory): mapper::set_flags to change page permissions"
```

---

## Task 2: Exec-memory allocator

**Files:**
- Create: `kernel/src/memory/exec.rs`
- Modify: `kernel/src/memory/mod.rs`

- [ ] **Step 1: Declare the module**

In `kernel/src/memory/mod.rs`, add to the module list (after `pub mod dma;`,
line 8):

```rust
pub mod exec;
```

- [ ] **Step 2: Write the allocator**

Create `kernel/src/memory/exec.rs`:

```rust
//! Executable-memory allocator (W^X) for AOT/JIT native code.
//!
//! A dedicated higher-half virtual window is bump-allocated per page. Frames are
//! aliased ONLY in this window (never given exec rights via the HHDM), so a page
//! is writable XOR executable, never both. Lifecycle: `alloc_exec` (writable,
//! NX) → write code → `protect_exec` (read-only, executable) → call → `free_exec`.

use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::{PhysAddr, VirtAddr};
use x86_64::structures::paging::PageTableFlags;
use crate::memory::{allocate_frame, free_frame, map_page, unmap_page, set_flags};

/// Base of the executable virtual window. Higher-half, canonical, and outside
/// the HHDM image and kernel sections. 4 KiB-granular bump allocation upward.
const EXEC_BASE: u64 = 0xFFFF_E000_0000_0000;
static NEXT: AtomicU64 = AtomicU64::new(EXEC_BASE);

/// A live executable allocation. `ptr` is the start of `pages * 4 KiB`.
pub struct ExecAlloc {
    pub ptr: *mut u8,
    pub len: usize,
    pages: usize,
}

#[derive(Debug)]
pub enum ExecError { NoFrame, Map, Protect }

/// Reserve `len` bytes (rounded up to whole pages) as writable, non-executable
/// memory for code emission. Write into `ptr`, then call `protect_exec`.
pub fn alloc_exec(len: usize) -> Result<ExecAlloc, ExecError> {
    let pages = (len + 0xFFF) / 0x1000;
    let base = NEXT.fetch_add((pages as u64) * 0x1000, Ordering::SeqCst);
    let wflags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::NO_EXECUTE;
    for i in 0..pages {
        // NOTE: if allocate_frame() returns PhysFrame instead of u64 (see Task 1
        // Step 1), use `frame.start_address()` here instead of `PhysAddr::new`.
        let phys_u64 = allocate_frame().ok_or(ExecError::NoFrame)?;
        let virt = VirtAddr::new(base + (i as u64) * 0x1000);
        let phys = PhysAddr::new(phys_u64);
        map_page(virt, phys, wflags).map_err(|_| ExecError::Map)?;
    }
    Ok(ExecAlloc { ptr: base as *mut u8, len, pages })
}

/// Flip the allocation to read-only + executable (W^X protect step).
pub fn protect_exec(a: &ExecAlloc) -> Result<(), ExecError> {
    // PRESENT only: not WRITABLE, NO_EXECUTE cleared → read + execute.
    let rxflags = PageTableFlags::PRESENT;
    let base = a.ptr as u64;
    for i in 0..a.pages {
        let virt = VirtAddr::new(base + (i as u64) * 0x1000);
        set_flags(virt, rxflags).map_err(|_| ExecError::Protect)?;
    }
    Ok(())
}

/// Unmap and free every frame of the allocation.
pub fn free_exec(a: ExecAlloc) {
    let base = a.ptr as u64;
    for i in 0..a.pages {
        let virt = VirtAddr::new(base + (i as u64) * 0x1000);
        if let Ok(frame) = unmap_page(virt) {
            // NOTE: adapt if free_frame takes a PhysFrame (Task 1 Step 1).
            free_frame(frame.start_address().as_u64());
        }
    }
}

/// Boot-check self-test: emit `mov eax,42; ret`, protect, call, expect 42.
#[cfg(feature = "boot-checks")]
pub fn self_test() -> bool {
    // x86-64: B8 2A 00 00 00 = mov eax,42 ; C3 = ret. (eax zero-extends into rax)
    let code: [u8; 6] = [0xB8, 0x2A, 0x00, 0x00, 0x00, 0xC3];
    let a = match alloc_exec(code.len()) {
        Ok(a) => a,
        Err(_) => return false,
    };
    // SAFETY: `a.ptr` covers `code.len()` writable bytes just mapped.
    unsafe { core::ptr::copy_nonoverlapping(code.as_ptr(), a.ptr, code.len()); }
    if protect_exec(&a).is_err() {
        return false;
    }
    // SAFETY: the bytes form a valid extern "C" fn() -> u64; pages are now RX.
    let f: extern "C" fn() -> u64 = unsafe { core::mem::transmute(a.ptr) };
    let r = f();
    free_exec(a);
    r == 42
}
```

- [ ] **Step 3: Run to verify it builds**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd <repo-on-wsl> && make iso'`
Expected: clean build. If `allocate_frame`/`free_frame` types differ, apply the
noted adaptations and rebuild.

- [ ] **Step 4: Commit**

```bash
git add kernel/src/memory/exec.rs kernel/src/memory/mod.rs
git commit -m "feat(memory): W^X executable-memory allocator (alloc/protect/free)"
```

---

## Task 3: Wire the self-test into boot

**Files:**
- Modify: `kernel/src/boot/phases/interrupts.rs`

The exec allocator needs the frame allocator + mapper fully initialised (done by
the `mem` phase, which runs before `interrupts`). Run the self-test in the
interrupts phase boot-check, alongside other checks.

- [ ] **Step 1: Add the self-test call**

In `kernel/src/boot/phases/interrupts.rs`, inside (or adjacent to) the
`#[cfg(feature = "boot-checks")]` block, add:

```rust
    #[cfg(feature = "boot-checks")]
    {
        let ok = crate::memory::exec::self_test();
        crate::binfo!("mem", "exec W^X self-test {}", if ok { "ok" } else { "FAIL" });
    }
```

- [ ] **Step 2: Prove the test is real (deliberate break), run, observe FAIL**

Temporarily change the expected value in `exec::self_test` from `r == 42` to
`r == 43`. Run:
`wsl -d Ubuntu -u root -e bash -c 'cd <repo-on-wsl> && make test-boot'`
Expected: log shows `mem: exec W^X self-test FAIL` (and boot continues / `$(HELLO)`
still appears — proving the executable code path actually ran and returned 42, so
the `== 43` check failed).

- [ ] **Step 3: Revert; run; observe PASS**

Restore `r == 42`. Run:
`wsl -d Ubuntu -u root -e bash -c 'cd <repo-on-wsl> && make test-boot'`
Expected: log shows `mem: exec W^X self-test ok` and `$(HELLO)`.

- [ ] **Step 4: Commit**

```bash
git add kernel/src/boot/phases/interrupts.rs
git commit -m "test(memory): boot-check W^X exec self-test (emit+protect+call)"
```

---

## Task 4: Changelog entry

**Files:**
- Create: `CHANGELOG/NN-26-06-04-exec-memory-wx.md` (next free `NN`)

- [ ] **Step 1: Next number**

Run: `ls CHANGELOG/ | sed 's/-.*//' | sort -n | tail -1` → use +1, zero-padded.

- [ ] **Step 2: Write the entry**

```markdown
# NN — Memoria eseguibile W^X

**Data:** 2026-06-04

## Cosa
Aggiunto allocatore di memoria eseguibile (W^X) su paging: `mapper::set_flags`
per cambiare permessi di pagine mappate, e `memory::exec` con
`alloc_exec`/`protect_exec`/`free_exec` su una finestra VA dedicata
(scrivibile+NX → flip a RX). Self-test boot-checks: emette `mov eax,42; ret`,
protegge, chiama, verifica 42.

## Perché
Prerequisito #2 del desktop egui: il runtime Wasmtime AOT (piano #3) carica
codice nativo in pagine RX. W^X = nessun VA scrivibile ed eseguibile insieme.

## File toccati
- kernel/src/memory/mapper.rs
- kernel/src/memory/exec.rs
- kernel/src/memory/mod.rs
- kernel/src/boot/phases/interrupts.rs
```

- [ ] **Step 3: Commit**

```bash
git add CHANGELOG/NN-26-06-04-exec-memory-wx.md
git commit -m "docs(changelog): W^X executable memory entry"
```

---

## Self-Review notes (already applied)

- **Spec coverage:** implements §5 "Exec memory W^X" platform-shim requirement
  and §13 prereq #2. The `ExecAlloc`/`alloc_exec`/`protect_exec`/`free_exec` API
  is exactly what plan #3's Wasmtime platform shim calls for code memory.
- **Types:** `set_flags(VirtAddr, PageTableFlags) -> Result<(), UnmapError>`,
  `alloc_exec(usize) -> Result<ExecAlloc, ExecError>`, `protect_exec(&ExecAlloc)`,
  `free_exec(ExecAlloc)` are consistent across tasks. Frame-type assumption is
  verified up front in Task 1 Step 1 with documented adaptation points.
- **Hazard noted:** `EXEC_BASE` must not collide with HHDM or kernel mappings;
  `0xFFFF_E000_…` is chosen in the higher-half hole. If `map_page` returns
  `AlreadyMapped` from the self-test, the window overlaps something — pick a
  different base. (Bump allocator never reuses VA in v1; no free-list — fine for
  spike-scale usage.)
- **icache:** x86-64 keeps L1I coherent with stores; the `update_flags` TLB
  flush also serialises. No explicit `cpuid`/`wbinvd` needed for correctness.
```
