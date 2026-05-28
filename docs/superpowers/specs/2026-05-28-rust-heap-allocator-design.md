# Rust Kernel Heap + Global Allocator — Design Spec

**Date:** 2026-05-28
**Milestone:** Step 4 of the Rust OS roadmap (`docs/superpowers/roadmap-rust-os.md`).
**Status:** Approved design, ready for implementation planning.

## Context

The kernel currently boots via Limine, prints to serial, and halts. It builds
`core` + `compiler_builtins` + `alloc` via `build-std`, but no
`#[global_allocator]` is wired, so `Box`, `Vec`, `String`, `BTreeMap`, etc. cannot
be used yet.

This milestone wires a real kernel heap on top of physical memory described by
Limine's memory map, using Limine's higher-half direct map (HHDM) to access it
virtually without any frame allocator or paging code of our own. After this
milestone, every later kernel layer (IDT/GDT structures, frame allocator
bookkeeping, scheduler, VFS) can freely use the `alloc` collections.

The Rust frame allocator + paging API (Step 6) will replace this minimal backing
with a managed region, reading the same memory map. This is intentional: Step 4
is the smallest thing that unlocks `alloc` for everything above it.

## Goals

- Wire a `#[global_allocator]` for the kernel so `alloc::` types work.
- Back it with **real, Limine-described RAM** (not a static BSS array), accessed via
  the Limine HHDM offset.
- Verify at boot that `Box` and `Vec` allocate, write, and read correctly, with
  results printed to serial.

## Non-goals (YAGNI)

- No growable / multi-chunk heap. One fixed-size region this milestone.
- No frame allocator, no paging code in Rust (Step 6).
- No multi-CPU contention handling beyond what the allocator already needs.
- No protection against `alloc` use before `init_heap()`.
- No allocation tracing/stats beyond the boot-time smoke test.

## Architecture

Three pieces:

1. **Limine requests** for the **memory map** and the **HHDM offset**, placed in
   the existing `.requests` link section bracketed by the existing markers.
2. A **`memory` module** that reads those responses, picks a free physical region
   of at least `HEAP_SIZE`, computes its virtual address as
   `virt = phys + hhdm_offset`, and hands that span to the global allocator.
3. A **global allocator** (`talc`) wrapped with `spin::Mutex` and an
   `ErrOnOom` handler, declared at the kernel-crate root so it is visible to
   `alloc`.

```
Limine -> memory map + HHDM offset
            |
            v
   memory::init_heap()
       - find first USABLE entry with length >= HEAP_SIZE
       - virt_base = phys_base + hhdm_offset
       - ALLOCATOR.lock().claim(Span::from_base_size(virt_base, HEAP_SIZE))
            |
            v
   #[global_allocator] talc::Talck   <- backs alloc::Box / Vec / String / BTreeMap
```

## Components

### 1. Limine requests (`kernel/src/main.rs`)

In addition to the existing `BASE_REVISION` request and start/end markers, add:

- A `MemoryMapRequest` static, `#[used] #[link_section = ".requests"]`.
- A `HhdmRequest` static, `#[used] #[link_section = ".requests"]`.

Both must live between `_START_MARKER` and `_END_MARKER` (already in place).

### 2. `kernel/src/memory.rs` (new module)

Constants:
- `pub const HEAP_SIZE: usize = 4 * 1024 * 1024;` (4 MiB).

Public API:

- `pub fn init_heap() -> Result<HeapInfo, HeapInitError>`
  - Reads the `MemoryMapRequest` response; returns `HeapInitError::NoMemoryMap`
    if absent.
  - Reads the `HhdmRequest` response; returns `HeapInitError::NoHhdm` if absent.
  - Scans the memory-map entries; picks the first whose type is **USABLE** and
    whose `length >= HEAP_SIZE`. Returns `HeapInitError::NoUsableRegion` if none.
  - Computes `virt_base = entry.base + hhdm.offset`.
  - Calls `ALLOCATOR.lock().claim(Span::from_base_size(virt_base as *mut u8,
    HEAP_SIZE))`. On failure returns `HeapInitError::ClaimFailed`.
  - On success returns `HeapInfo { phys_base, virt_base, size: HEAP_SIZE }`.

- `pub struct HeapInfo { pub phys_base: u64, pub virt_base: u64, pub size: usize }`
- `pub enum HeapInitError { NoMemoryMap, NoHhdm, NoUsableRegion, ClaimFailed }`
  - `impl fmt::Display for HeapInitError` so callers can log a human string.

Internal:
- The global allocator lives in this module:
  `#[global_allocator] static ALLOCATOR: Talck<spin::Mutex<()>, ErrOnOom> = ...;`

### 3. `Cargo.toml` (kernel crate)

Add dependencies `talc` and `spin`. Exact versions pinned during planning; commit
the updated `Cargo.lock`.

### 4. `kernel/src/main.rs` — boot wiring

In `kmain`, after the existing serial init and hello line:

1. `extern crate alloc;` at the crate root.
2. `mod memory;`.
3. Call `memory::init_heap()`.
   - On `Ok(info)`: write
     `MinimalOS-rs: heap ok base=0x<virt_base:x> size=<size>\n` to serial.
   - On `Err(e)`: write `MinimalOS-rs: heap fail: <Display>\n` and halt.
4. Smoke test:
   - `let b = alloc::boxed::Box::new(0xCAFEBABEu64);`
   - `let v: alloc::vec::Vec<u32> = (0..5).collect();`
   - Write `MinimalOS-rs: alloc box=0x<*b:x> vec=[v0,v1,...]\n` to serial.
5. Halt (existing `hcf()`).

### 5. Build/test wiring (`Makefile`)

The `HELLO` Makefile variable currently asserts the hello line. Change the
asserted string to the new "alloc box=0xCAFEBABE" line, since the hello line is a
prerequisite that is verified implicitly (it would not have reached the alloc
test if hello had failed). Or grep for both — the plan resolves this concretely.

## Data flow

1. Limine fills the `MemoryMapRequest` and `HhdmRequest` responses before
   transferring control to `kmain`.
2. `kmain` initializes serial and emits hello.
3. `memory::init_heap()` reads both responses, picks a region, and claims it
   into `talc`.
4. From this point, any `alloc::` collection works.
5. The smoke test allocates a `Box` and a `Vec`, prints their effects, halts.

## Error handling

Every failure mode is observable on the serial wire:

- `heap fail: no memory map`
- `heap fail: no hhdm`
- `heap fail: no usable region (need <SIZE> bytes)`
- `heap fail: claim`

After a heap failure the kernel writes the diagnostic and halts (`hcf()`).
There is no fallback heap.

## Testing

- **Build:** `make iso` succeeds.
- **Runtime (the test):** `make run-test` boots headless, captures serial, and
  asserts the log contains the alloc-test signature. Expected serial:
  ```
  MinimalOS-rs: hello serial
  MinimalOS-rs: heap ok base=0x... size=4194304
  MinimalOS-rs: alloc box=0xCAFEBABE vec=[0,1,2,3,4]
  ```
- **Negative paths** (no usable region, no hhdm) are not actively triggered in
  the test environment but are exercised by code review of the error branches.

## Open items for the implementation plan

- Pin `talc` and `spin` versions and confirm the chosen `Talck`/`ErrOnOom` API
  against the resolved version (the talc API has evolved across 4.x releases).
- Confirm the limine crate (0.6.3) types for `MemoryMapRequest` / `HhdmRequest` /
  the USABLE entry-type enum and adapt code to whatever names that version
  exposes.
- Decide whether `make run-test` asserts the hello line, the alloc line, or
  both (suggest the alloc line plus a comment that the hello line is implicit).
