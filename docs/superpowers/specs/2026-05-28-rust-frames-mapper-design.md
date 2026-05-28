# Rust Frame Allocator + Paging Mapper API — Design Spec

**Date:** 2026-05-28
**Milestone:** Step 6 of the Rust OS roadmap (`docs/superpowers/roadmap-rust-os.md`).
**Status:** Approved design, ready for implementation planning.

## Context

The kernel currently has a 4 MiB heap (`talc`) on a Limine USABLE region, a
working APIC/timer/keyboard stack, and an ad-hoc MMIO mapper in
`kernel/src/apic/mmio.rs`. The MMIO mapper walks the existing page tables
manually, allocates intermediate page-table pages by `Box::leak`-ing them from
the kernel heap, and writes leaf entries directly. It carries a hand-rolled
`HUGE_PAGE` guard.

This is fine for two MMIO regions but does not generalize. The kernel needs a
proper physical frame allocator (so future code — Step 7 VFS in tmpfs, Step 10
WASM memory growth, Step 13 framebuffer DMA, Step 14 virtio rings — can ask
for pages) and a single paging API built on `x86_64::structures::paging` so
every mapping goes through the same typed path with the same safety guarantees.

The post-pivot roadmap rules out per-process page tables and ring 3, so Step 6
is "kernel-system paging" only: one address space, one Mapper, frames handed
out for kernel use.

## Goals

- Physical frame allocator backed by a bitmap stored on the kernel heap,
  initialized from the Limine memory map.
- `FrameAllocator<Size4KiB>` + `FrameDeallocator<Size4KiB>` implementations
  usable by `x86_64::Mapper`-based code.
- A single global Mapper (`OffsetPageTable`) wrapper that exposes
  `map_page`, `unmap_page`, and `map_io_page` (UC for MMIO).
- The existing `apic/mmio.rs` ad-hoc walker is removed; `lapic::init` and
  `ioapic::init` use the new `map_io_page` instead.
- Boot-time smoke test: map a fresh high virtual address, write/read a u64,
  unmap, free the frame. Logged on serial.
- `TEST_PASS` (`make run-test`) keeps passing: the asserted line stays
  `ruos: ticks=`, but it now also implicitly proves the new frame allocator
  and Mapper are working (LAPIC/IOAPIC MMIO is mapped through them).

## Non-goals (YAGNI)

- No per-process page tables, no ring 3, no SYSCALL/SYSRET MSRs.
- No huge-page (2 MiB / 1 GiB) allocations — only 4 KiB.
- No swap, no demand paging, no copy-on-write.
- No reclamation of Limine `BOOTLOADER_RECLAIMABLE` regions yet.
- No unmap-at-teardown logic (the kernel never quits).
- No NUMA awareness, no per-CPU allocators.

## Architecture

Three layers in a single new sub-tree `kernel/src/memory/`:

```
Limine memory map  +  HHDM offset
        |
        v
  frames.rs  (bitmap on the heap)
        |
        | impl FrameAllocator<Size4KiB> + FrameDeallocator<Size4KiB>
        v
  mapper.rs  (OffsetPageTable wrapper)
        |
        | helpers: map_page / unmap_page / map_io_page
        v
  consumers: lapic::init, ioapic::init (refactored to call helpers),
             future Steps (VFS, WASM, framebuffer, virtio)
```

The current `kernel/src/memory.rs` (heap + global allocator) is split:

```
kernel/src/memory/
  mod.rs    # crate-facing API: re-exports + init_paging()
  heap.rs   # existing talc allocator + init_heap() (moved as-is)
  frames.rs # new bitmap frame allocator
  mapper.rs # new OffsetPageTable wrapper
```

Public API exposed by `memory::mod`:

- `pub use heap::{ALLOCATOR, HEAP_SIZE, HeapInfo, HeapInitError, init_heap, heap_region}`
- `pub use frames::{init as init_frames, free_frame, frame_counts}`
- `pub use mapper::{init as init_mapper, map_page, unmap_page, map_io_page}`

## Components

### 1. `memory/frames.rs` — bitmap frame allocator

Data:

```rust
pub struct Frames {
    bitmap: Vec<u64>,   // 1 bit per 4 KiB frame; bit=1 means USED
    total:  u64,        // bitmap.len() * 64
    used:   u64,
}

static FRAMES: spin::Mutex<Option<Frames>> = spin::Mutex::new(None);
```

Init algorithm:

1. Walk the Limine memory map. Find `max_phys = max(entry.base + entry.length)`
   across **all** entries (not just USABLE — we want the bitmap to be addressable
   up to wherever physical memory ends; non-USABLE bits stay marked USED).
2. Allocate the bitmap on the heap with all bits set to `1` (USED).
3. For each USABLE entry, mark the bits covering its frame range as `0` (FREE).
   Round `base` up to the next 4 KiB boundary and `base+length` down.
4. Re-mark the heap region (`heap_region()`) as `1` so frames already owned by
   the heap allocator are not handed out again.
5. Store the populated `Frames` into `FRAMES`.

API:

```rust
pub fn init(hhdm_offset: u64) -> Result<FrameCounts, FrameInitError>;
pub fn free_frame(frame: PhysFrame<Size4KiB>);
pub fn frame_counts() -> FrameCounts; // { total, used, free }

#[derive(Debug, Copy, Clone)]
pub struct FrameCounts { pub total: u64, pub used: u64, pub free: u64 }

#[derive(Debug)]
pub enum FrameInitError { NoMemoryMap, NoUsableRegion, BitmapAllocFailed }
impl core::fmt::Display for FrameInitError { ... }
```

`Frames` also implements (in `frames.rs`, used by `mapper.rs`):

```rust
unsafe impl FrameAllocator<Size4KiB> for &mut Frames { ... }
impl FrameDeallocator<Size4KiB> for &mut Frames { ... }
```

The `FrameAllocator` trait is consumed via a short helper that locks `FRAMES`
and applies a closure — `mapper.rs` will use this for `map_page`'s intermediate
table allocations.

### 2. `memory/mapper.rs` — Mapper wrapper

```rust
static MAPPER: spin::Mutex<Option<OffsetPageTable<'static>>> = spin::Mutex::new(None);

pub fn init(hhdm_offset: u64) -> Result<(), MapperInitError>;
pub fn map_page(virt: VirtAddr, phys: PhysAddr, flags: PageTableFlags)
    -> Result<(), MapError>;
pub fn unmap_page(virt: VirtAddr) -> Result<PhysFrame<Size4KiB>, UnmapError>;
pub fn map_io_page(phys: PhysAddr) -> Result<VirtAddr, MapError>;
// returns phys + hhdm_offset; UC flags (PCD|PWT|RW|PRESENT|NX).
```

`init` builds the global `OffsetPageTable` by reading CR3, converting the
PML4 physical address to a `&'static mut PageTable` via the HHDM offset, then
wrapping it with `OffsetPageTable::new(pml4, VirtAddr::new(hhdm_offset))`.

`map_page`/`unmap_page` lock both `MAPPER` and `FRAMES` (briefly) and call the
underlying `Mapper::map_to` / `Mapper::unmap`. They translate the crate's
`Result` types into a project-local `MapError` enum so consumers don't depend
on `x86_64` types beyond `VirtAddr`/`PhysAddr`/`PageTableFlags`.

`map_io_page(phys)` is a convenience that:
- Computes `virt = phys + hhdm_offset`.
- Calls `map_page(virt, phys, NO_EXECUTE | WRITABLE | WRITE_THROUGH | NO_CACHE | PRESENT)`.
- Returns the virtual address.

The `x86_64` `Mapper` already returns `MapToError::ParentEntryHugePage` if a
walk would descend through a 2 MiB / 1 GiB leaf — the hand-rolled `HUGE_PAGE`
guard in the old `mmio.rs` is no longer needed.

### 3. `memory/mod.rs` — orchestrator

Public re-exports + an `init_paging(hhdm_offset)` that runs frames init →
mapper init and logs counts. Existing `heap.rs` re-exported unchanged.

### 4. Refactor `apic/mmio.rs`

Reduced to a thin shim or deleted entirely. The two call sites
(`lapic::init`, `ioapic::init`) call `memory::map_io_page(PhysAddr::new(base))`
and use the returned `VirtAddr` for their MMIO accesses. The `LEAKED` counter,
`next_table_or_create`, and the manual `HUGE_PAGE` panic guard are removed —
their semantics now live inside the typed `Mapper` API.

### 5. `kmain` boot sequence (additions)

After `acpi_init::parse()` (which is the source of `hhdm_offset`) and before
`apic::lapic::init`:

```rust
let hhdm = acpi_info.hhdm_offset;
let frame_counts = memory::init_frames(hhdm).expect("frames init");
memory::init_mapper(hhdm).expect("mapper init");
kprintln!(
    "ruos: paging up frames total={} used={} free={}",
    frame_counts.total, frame_counts.used, frame_counts.free,
);

// Smoke test: map a fresh high VA, write/read, unmap.
let test_virt = VirtAddr::new(0x4000_0000_0000);
let frame = {
    let mut g = memory::frames::FRAMES.lock();
    g.as_mut().unwrap().allocate_frame().expect("test frame")
};
memory::map_page(test_virt, frame.start_address(),
    PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE)
    .expect("test map");
unsafe { test_virt.as_mut_ptr::<u64>().write_volatile(0xC0FFEEu64); }
let back = unsafe { test_virt.as_ptr::<u64>().read_volatile() };
assert_eq!(back, 0xC0FFEE);
memory::unmap_page(test_virt).expect("test unmap");
memory::free_frame(frame);
kprintln!(
    "ruos: map test ok virt=0x{:X} phys=0x{:X}",
    test_virt.as_u64(), frame.start_address().as_u64(),
);
```

The rest of `kmain` (LAPIC, IOAPIC, timer, keyboard, sti, busy-wait,
`ruos: ticks=N`, halt) is unchanged.

## Data flow

```
Limine RSDP/HHDM responses
        |
        v
acpi_init::parse  -> AcpiInfo { hhdm_offset, lapic_base, ioapic_base, overrides }
        |
        v
memory::init_frames(hhdm)         -- bitmap built on heap
memory::init_mapper(hhdm)         -- OffsetPageTable bound to current CR3
        |
        v
smoke test: map(0x4000000000, frame) -> RW -> unmap -> free
        |
        v
apic::lapic::init(lapic_base, hhdm, ...)
   -> memory::map_io_page(lapic_base) -> virt for SVR/EOI/timer regs
apic::ioapic::init(ioapic_base, hhdm)
   -> memory::map_io_page(ioapic_base) -> virt for IOREGSEL/IOWIN
```

## Error handling

Every failure path writes a distinct line on serial via `kprintln!` and halts:

- `ruos: frames fail: no memory map` — `MemmapRequest` response missing.
- `ruos: frames fail: no usable region` — every memmap entry non-USABLE.
- `ruos: frames fail: bitmap alloc` — heap couldn't allocate the bitmap.
- `ruos: mapper fail: parse cr3` — CR3 read returned bogus PML4 (very rare).
- `ruos: map test failed: <err>` — the boot-time smoke test panicked (kernel
  panic; `panic_handler` halts).
- `ruos: map_io_page failed: <err>` from `lapic::init` / `ioapic::init` —
  surfaces underlying `x86_64::MapToError` (e.g. `ParentEntryHugePage`).

## Testing

- **Automated (`make run-test`):** asserts `ruos: ticks=` as before. Reaching
  it implies the full chain: heap → frames → mapper → MMIO → APIC → timer →
  `sti`. The smoke-test line `ruos: map test ok` appears earlier in the log;
  reviewing `build/serial.log` confirms it.
- **Manual:** VirtualBox / QEMU display boot — visually inspect the new lines
  `ruos: paging up frames total=N used=M free=K` and
  `ruos: map test ok virt=... phys=...`.
- **Negative paths** (no memmap, smoke-test write-mismatch, huge-page parent)
  are exercised by code review of the error branches.

## Decomposition into tasks

The plan will turn this into three tasks (TDD-style: build green at each
checkpoint, TEST_PASS preserved after each):

1. **Frame allocator** — split `memory.rs` into `memory/mod.rs` + `memory/heap.rs`
   (move existing code) + create `memory/frames.rs`. Init in `kmain` after the
   ACPI log. Logs `ruos: frames usable=...`. Old `apic/mmio.rs` still active.
2. **Mapper wrapper + smoke test** — create `memory/mapper.rs`, init after
   frames. Run the boot-time `map`/`unmap` smoke test on `0x4000000000`. Logs
   `ruos: paging up` and `ruos: map test ok`. Old `apic/mmio.rs` still active.
3. **Refactor `apic/mmio.rs`** — replace its callers with
   `memory::map_io_page`. Delete `apic/mmio.rs`. TEST_PASS preserved (LAPIC +
   IOAPIC still reachable through the new path).

## Open items for the implementation plan

- Exact `MapError`/`UnmapError` enum shape (which subset of
  `x86_64::Mapper`'s error variants to surface).
- Whether the bitmap chunk type is `u64` (one 64-frame chunk = 256 KiB of RAM)
  or `usize` — irrelevant on x86_64-unknown-none where both are 64 bits;
  pick `u64` for explicitness.
- Final naming of the heap-region exclusion helper (`heap_region()` is already
  in `heap.rs` — reuse vs introduce a shared "reserved regions" list).
- Concrete `kmain` smoke-test pseudocode currently dereferences
  `FRAMES.lock().as_mut().unwrap()` directly; the plan should wrap this in a
  small `frames::allocate_frame()` helper to keep `kmain` clean.
