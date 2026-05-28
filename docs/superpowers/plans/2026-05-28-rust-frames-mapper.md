# Rust Frame Allocator + Mapper Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a bitmap physical frame allocator + a typed `OffsetPageTable`-based Mapper API, and migrate the ad-hoc `apic/mmio.rs` page-walk to that Mapper.

**Architecture:** Split `kernel/src/memory.rs` into a `memory/` module tree (`mod.rs`, `heap.rs`, `frames.rs`, `mapper.rs`). The frame allocator stores a bitmap on the heap, initialized from the Limine memory map. The Mapper wraps `x86_64::structures::paging::OffsetPageTable` driven by the HHDM offset and consumes the frame allocator through the `FrameAllocator<Size4KiB>` trait. `apic/mmio.rs` collapses to a `memory::map_io_page` call.

**Tech Stack:** Rust nightly `nightly-2026-05-26`, `x86_64 0.15.4`, `spin 0.9.8`, existing `talc`/`limine`/`acpi`/`uart_16550`. WSL Ubuntu host.

---

## Key facts

- All build/run via **WSL Ubuntu** as root, cargo env sourced:
  ```
  wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && <cmd>'
  ```
  Edit files with Edit/Write on Windows paths. Git in normal shell. Branch `feature/frames-mapper`. Do not push, do not skip hooks.
- The current kernel: heap (talc) + GDT/TSS + IDT + ACPI parsed + LAPIC/IOAPIC (custom MMIO via `apic/mmio.rs`) + timer 100 Hz + keyboard. Makefile asserts `ruos: ticks=`.
- **Spec:** `docs/superpowers/specs/2026-05-28-rust-frames-mapper-design.md`.
- Limine memory map exposes `entries()` of `&Entry` with `base: u64`, `length: u64`, `type_: u64` (constant `limine::memmap::MEMMAP_USABLE = 0`).
- AcpiInfo already carries `hhdm_offset`. Reuse it; do NOT add another HHDM Limine request.
- The current `kmain` already calls `gdt::init()`, `idt::init()`, `int3` smoke, `pic::disable()`, `acpi_init::parse()` logging `ruos: acpi ok ...`. New code lands AFTER that log and BEFORE `apic::lapic::init`.

## File structure (target)

After all three tasks:

```
kernel/src/memory/
  mod.rs    # crate-facing re-exports + init_paging() orchestrator
  heap.rs   # existing memory.rs content (talc + HEAP_SIZE + init_heap + heap_region)
  frames.rs # new bitmap frame allocator + FrameAllocator/Deallocator impls
  mapper.rs # new OffsetPageTable wrapper + map_page/unmap_page/map_io_page
kernel/src/apic/
  mod.rs    # `pub mod lapic; pub mod ioapic;` (mmio submodule removed)
  lapic.rs  # uses memory::map_io_page; loses hhdm_offset parameter
  ioapic.rs # uses memory::map_io_page; loses hhdm_offset parameter
  # (mmio.rs deleted)
```

---

## Task 1: Module split + frame allocator

**Files:**
- Delete: `kernel/src/memory.rs`
- Create: `kernel/src/memory/mod.rs`
- Create: `kernel/src/memory/heap.rs`
- Create: `kernel/src/memory/frames.rs`
- Modify: `kernel/src/main.rs`
- Create: `CHANGELOG/34-26-05-28-frame-allocator.md`

- [ ] **Step 1: Create `kernel/src/memory/heap.rs` with the moved heap code**

Copy the current content of `kernel/src/memory.rs` verbatim into a new file `kernel/src/memory/heap.rs`. No content changes — just relocate. (After Step 3 the old file is deleted.)

The relocated file is the existing heap module: `ALLOCATOR`, `HEAP_SIZE`, `HeapInfo`, `HeapInitError` (with `Display`), `HEAP_INFO`, `heap_region`, `init_heap`.

- [ ] **Step 2: Create `kernel/src/memory/mod.rs`**

```rust
//! Memory subsystem: heap (talc), physical frame allocator (bitmap), and
//! paging Mapper. Re-exports the most commonly used names so callers see a
//! single `crate::memory::*` API regardless of internal layout.

pub mod heap;
pub mod frames;

pub use heap::{ALLOCATOR, HEAP_SIZE, HeapInfo, HeapInitError, init_heap, heap_region};
pub use frames::{FrameCounts, FrameInitError, allocate_frame, free_frame, frame_counts,
    init as init_frames};
```

- [ ] **Step 3: Delete the old `kernel/src/memory.rs`**

```bash
git rm kernel/src/memory.rs
```

The new module tree (`memory/mod.rs` + `memory/heap.rs`) now serves the same path. Existing call sites that used `crate::memory::init_heap()`, `crate::memory::ALLOCATOR`, etc. resolve unchanged through the `pub use` re-exports.

- [ ] **Step 4: Create `kernel/src/memory/frames.rs`**

```rust
//! Bitmap physical frame allocator.
//!
//! The bitmap is sized to cover every physical address the Limine memory map
//! mentions (so we can address the highest-numbered USABLE frame). Each bit is
//! 1 = used, 0 = free. The bitmap itself lives on the kernel heap.
//!
//! At init: every bit starts USED, then USABLE memmap entries are walked to
//! mark their frames FREE, then the heap region (already owned by talc) is
//! re-marked USED so we never hand the heap's backing frames back out.

use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use limine::memmap::MEMMAP_USABLE;
use x86_64::PhysAddr;
use x86_64::structures::paging::{FrameAllocator, FrameDeallocator, PhysFrame, Size4KiB};

pub const PAGE_SIZE: u64 = 4096;

#[derive(Debug, Copy, Clone)]
pub struct FrameCounts {
    pub total: u64,
    pub used:  u64,
    pub free:  u64,
}

#[derive(Debug)]
pub enum FrameInitError {
    NoMemoryMap,
    NoUsableRegion,
}

impl fmt::Display for FrameInitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameInitError::NoMemoryMap    => f.write_str("no memory map"),
            FrameInitError::NoUsableRegion => f.write_str("no usable region"),
        }
    }
}

pub struct Frames {
    bitmap: Vec<u64>,
    total:  u64,
    used:   u64,
}

impl Frames {
    fn new(total: u64) -> Self {
        let chunks = ((total + 63) / 64) as usize;
        Frames { bitmap: vec![u64::MAX; chunks], total, used: total }
    }

    #[inline]
    fn idx(frame: u64) -> (usize, u32) {
        ((frame / 64) as usize, (frame % 64) as u32)
    }

    fn mark_used(&mut self, frame: u64) {
        if frame >= self.total { return; }
        let (i, b) = Self::idx(frame);
        if (self.bitmap[i] >> b) & 1 == 0 {
            self.bitmap[i] |= 1u64 << b;
            self.used += 1;
        }
    }

    fn mark_free(&mut self, frame: u64) {
        if frame >= self.total { return; }
        let (i, b) = Self::idx(frame);
        if (self.bitmap[i] >> b) & 1 == 1 {
            self.bitmap[i] &= !(1u64 << b);
            self.used -= 1;
        }
    }

    fn counts(&self) -> FrameCounts {
        FrameCounts { total: self.total, used: self.used, free: self.total - self.used }
    }
}

unsafe impl FrameAllocator<Size4KiB> for Frames {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        for (i, chunk) in self.bitmap.iter_mut().enumerate() {
            if *chunk != u64::MAX {
                let bit = chunk.trailing_ones();
                let frame = (i as u64) * 64 + (bit as u64);
                if frame >= self.total { return None; }
                *chunk |= 1u64 << bit;
                self.used += 1;
                return Some(PhysFrame::containing_address(PhysAddr::new(frame * PAGE_SIZE)));
            }
        }
        None
    }
}

impl FrameDeallocator<Size4KiB> for Frames {
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        let f = frame.start_address().as_u64() / PAGE_SIZE;
        self.mark_free(f);
    }
}

pub static FRAMES: spin::Mutex<Option<Frames>> = spin::Mutex::new(None);

pub fn init() -> Result<FrameCounts, FrameInitError> {
    let memmap = crate::MEMMAP_REQUEST
        .response()
        .ok_or(FrameInitError::NoMemoryMap)?;

    // Highest physical address mentioned by ANY entry — we need the bitmap
    // big enough to talk about frames at the top of RAM. Non-USABLE bits stay
    // at 1 (used).
    let mut max_phys: u64 = 0;
    let mut has_usable = false;
    for entry in memmap.entries().iter() {
        let end = entry.base + entry.length;
        if end > max_phys { max_phys = end; }
        if entry.type_ == MEMMAP_USABLE { has_usable = true; }
    }
    if !has_usable { return Err(FrameInitError::NoUsableRegion); }

    let total_frames = (max_phys + PAGE_SIZE - 1) / PAGE_SIZE;
    let mut frames = Frames::new(total_frames);

    // Free every frame fully inside a USABLE entry.
    for entry in memmap.entries().iter() {
        if entry.type_ != MEMMAP_USABLE { continue; }
        let first = (entry.base + PAGE_SIZE - 1) / PAGE_SIZE;  // round up
        let last  = (entry.base + entry.length) / PAGE_SIZE;    // round down
        for f in first..last {
            frames.mark_free(f);
        }
    }

    // Heap frames are owned by talc — do not hand them back out.
    if let Some(info) = crate::memory::heap::heap_region() {
        let first = info.phys_base / PAGE_SIZE;
        let last  = (info.phys_base + info.size as u64 + PAGE_SIZE - 1) / PAGE_SIZE;
        for f in first..last {
            frames.mark_used(f);
        }
    }

    let counts = frames.counts();
    *FRAMES.lock() = Some(frames);
    Ok(counts)
}

pub fn allocate_frame() -> Option<PhysFrame<Size4KiB>> {
    FRAMES.lock().as_mut().and_then(|f| f.allocate_frame())
}

pub fn free_frame(frame: PhysFrame<Size4KiB>) {
    if let Some(f) = FRAMES.lock().as_mut() {
        unsafe { f.deallocate_frame(frame); }
    }
}

pub fn frame_counts() -> FrameCounts {
    FRAMES.lock()
        .as_ref()
        .map(|f| f.counts())
        .unwrap_or(FrameCounts { total: 0, used: 0, free: 0 })
}
```

(If `limine::memmap::MEMMAP_USABLE` is at a different path in the resolved version, adjust the `use` line minimally — same as the working call site in `kernel/src/acpi_init.rs`. The `FrameAllocator`/`FrameDeallocator` traits are stable across `x86_64` 0.15.x.)

- [ ] **Step 5: Wire `init_frames` into `kmain`**

Edit `kernel/src/main.rs`. After the existing `kprintln!("ruos: acpi ok ...")` line and before `apic::lapic::init(...)`, add:

```rust
    let frame_counts = match memory::init_frames() {
        Ok(c) => c,
        Err(e) => {
            kprintln!("ruos: frames fail: {}", e);
            hcf();
        }
    };
    kprintln!(
        "ruos: frames total={} used={} free={}",
        frame_counts.total, frame_counts.used, frame_counts.free,
    );
```

(`memory::init_frames` is the `pub use frames::init as init_frames` re-export added in Step 2.)

- [ ] **Step 6: Build and run**

```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -12'
```
Expected serial includes a new line, then `TEST_PASS`:
```
ruos: acpi ok lapic=0x... ioapic=0x... overrides=N
ruos: frames total=<N> used=<M> free=<K>
ruos: lapic calibrated ... ticks/sec ...
ruos: ticks=<N>
```
`total - used = free` should hold. On QEMU `-m 512` expect `total ≈ 131072` (= 512 MiB / 4 KiB). LAPIC + IOAPIC continue to work via the still-active `apic/mmio.rs` (refactored in Task 3).

- [ ] **Step 7: Changelog**

Create `CHANGELOG/34-26-05-28-frame-allocator.md`:

```markdown
# 34 — Frame allocator fisico (bitmap, da Limine memmap)

**Data:** 2026-05-28

## Cosa
- `kernel/src/memory.rs` → split in `kernel/src/memory/mod.rs` + `heap.rs`
  (contenuto invariato) + `frames.rs` (nuovo).
- `frames.rs`: bitmap su heap, sized da `max_phys` Limine, USABLE → free,
  heap region → re-marcata used. Impl `FrameAllocator<Size4KiB>` +
  `FrameDeallocator<Size4KiB>` (trait `x86_64`).
- API: `init() -> Result<FrameCounts, FrameInitError>`, `allocate_frame()`,
  `free_frame()`, `frame_counts()`, static `FRAMES: spin::Mutex<Option<Frames>>`.
- `kmain`: chiama `memory::init_frames()` dopo `acpi_init::parse()`, logga
  `ruos: frames total=N used=M free=K`. `apic/mmio.rs` ancora attivo.

## Perché
Primo pezzo dello Step 6: avere un produttore di frame fisici prima di
costruire il Mapper (Task 2).

## File toccati
- kernel/src/memory.rs (rimosso)
- kernel/src/memory/mod.rs (nuovo)
- kernel/src/memory/heap.rs (nuovo, contenuto spostato)
- kernel/src/memory/frames.rs (nuovo)
- kernel/src/main.rs
- CHANGELOG/34-26-05-28-frame-allocator.md
```

- [ ] **Step 8: Commit**

```bash
git add kernel/src/memory kernel/src/main.rs CHANGELOG/34-26-05-28-frame-allocator.md
git add -u kernel/src/memory.rs
git commit -m "feat(rust): bitmap frame allocator + memory/ module split"
```

(`git add -u` picks up the `git rm` of the old file.)

---

## Task 2: Mapper wrapper + boot-time smoke test

**Files:**
- Create: `kernel/src/memory/mapper.rs`
- Modify: `kernel/src/memory/mod.rs`
- Modify: `kernel/src/main.rs`
- Create: `CHANGELOG/35-26-05-28-mapper-api.md`

- [ ] **Step 1: Create `kernel/src/memory/mapper.rs`**

```rust
//! Paging Mapper: a single global `OffsetPageTable` driven by Limine's HHDM
//! offset, plus thin helpers used everywhere outside this module.

use core::fmt;
use spin::Mutex;
use x86_64::{PhysAddr, VirtAddr};
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::{
    OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Mapper, Size4KiB,
};
use x86_64::structures::paging::mapper::{MapToError, UnmapError as XUnmapError};

static MAPPER: Mutex<Option<OffsetPageTable<'static>>> = Mutex::new(None);
static HHDM_OFFSET: spin::Once<u64> = spin::Once::new();

#[derive(Debug)]
pub enum MapError {
    NotInitialized,
    AlreadyMapped,
    NoFrame,
    ParentHugePage,
}

#[derive(Debug)]
pub enum UnmapError {
    NotInitialized,
    NotMapped,
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MapError::NotInitialized => f.write_str("not initialized"),
            MapError::AlreadyMapped  => f.write_str("already mapped"),
            MapError::NoFrame        => f.write_str("no frame"),
            MapError::ParentHugePage => f.write_str("parent huge-page"),
        }
    }
}

impl fmt::Display for UnmapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnmapError::NotInitialized => f.write_str("not initialized"),
            UnmapError::NotMapped      => f.write_str("not mapped"),
        }
    }
}

pub fn init(hhdm_offset: u64) {
    HHDM_OFFSET.call_once(|| hhdm_offset);
    let (cr3_frame, _) = Cr3::read();
    let pml4_virt = cr3_frame.start_address().as_u64() + hhdm_offset;
    // SAFETY: `pml4_virt` is the HHDM image of the live PML4. We become the
    // sole writer to `MAPPER` for the lifetime of the kernel; the underlying
    // page tables are mutated only through the Mapper API.
    let pml4: &'static mut PageTable = unsafe { &mut *(pml4_virt as *mut PageTable) };
    let table = unsafe { OffsetPageTable::new(pml4, VirtAddr::new(hhdm_offset)) };
    *MAPPER.lock() = Some(table);
}

pub fn map_page(virt: VirtAddr, phys: PhysAddr, flags: PageTableFlags)
    -> Result<(), MapError>
{
    let mut g_map = MAPPER.lock();
    let mapper = g_map.as_mut().ok_or(MapError::NotInitialized)?;
    let mut g_frames = crate::memory::frames::FRAMES.lock();
    let frames = g_frames.as_mut().ok_or(MapError::NoFrame)?;

    let page: Page<Size4KiB> = Page::containing_address(virt);
    let frame: PhysFrame<Size4KiB> = PhysFrame::containing_address(phys);

    // SAFETY: caller is responsible for the semantic safety of the mapping
    // (e.g. not aliasing kernel-only memory into a hostile owner). We only
    // wire phys/virt and let the typed Mapper reject structural errors.
    unsafe {
        mapper.map_to(page, frame, flags, frames)
            .map_err(|e| match e {
                MapToError::FrameAllocationFailed => MapError::NoFrame,
                MapToError::PageAlreadyMapped(_)  => MapError::AlreadyMapped,
                MapToError::ParentEntryHugePage   => MapError::ParentHugePage,
            })?
            .flush();
    }
    Ok(())
}

pub fn unmap_page(virt: VirtAddr) -> Result<PhysFrame<Size4KiB>, UnmapError> {
    let mut g_map = MAPPER.lock();
    let mapper = g_map.as_mut().ok_or(UnmapError::NotInitialized)?;
    let page: Page<Size4KiB> = Page::containing_address(virt);
    let (frame, flush) = mapper.unmap(page).map_err(|e| match e {
        XUnmapError::PageNotMapped => UnmapError::NotMapped,
        _ => UnmapError::NotMapped,
    })?;
    flush.flush();
    Ok(frame)
}

pub fn map_io_page(phys: PhysAddr) -> Result<VirtAddr, MapError> {
    let hhdm = *HHDM_OFFSET.get().ok_or(MapError::NotInitialized)?;
    let virt = VirtAddr::new(phys.as_u64() + hhdm);
    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::WRITE_THROUGH
        | PageTableFlags::NO_CACHE
        | PageTableFlags::NO_EXECUTE;
    match map_page(virt, phys, flags) {
        Ok(()) => Ok(virt),
        // Idempotent: if a previous mapping already exists (e.g. Limine HHDM
        // covers this MMIO via a huge page, or we are called twice), accept.
        Err(MapError::AlreadyMapped) => Ok(virt),
        Err(e) => Err(e),
    }
}
```

(The MMIO path may collide with Limine's HHDM if Limine uses huge pages at
that range — the typed Mapper will return `MapError::ParentHugePage`. This is
the known limitation already noted in the spec; long-term fix is a dedicated
non-HHDM virtual range for MMIO. For now, on QEMU/VBox with our test sizes,
the MMIO addresses fall in absent PDPT entries and the map succeeds.)

- [ ] **Step 2: Add `mapper` to `memory/mod.rs`**

Edit `kernel/src/memory/mod.rs` and append:

```rust
pub mod mapper;
pub use mapper::{MapError, UnmapError, init as init_mapper, map_page, unmap_page, map_io_page};
```

- [ ] **Step 3: Wire `init_mapper` and run the boot-time smoke test in `kmain`**

Edit `kernel/src/main.rs`. The block that currently reads:

```rust
    let frame_counts = match memory::init_frames() {
        Ok(c) => c,
        Err(e) => {
            kprintln!("ruos: frames fail: {}", e);
            hcf();
        }
    };
    kprintln!(
        "ruos: frames total={} used={} free={}",
        frame_counts.total, frame_counts.used, frame_counts.free,
    );
```

is followed by a new block (still BEFORE `apic::lapic::init`):

```rust
    memory::init_mapper(acpi_info.hhdm_offset);
    kprintln!("ruos: paging up");

    // Smoke test: map a fresh canonical lower-half VA, write/read, unmap.
    {
        use x86_64::structures::paging::PageTableFlags;
        let test_virt = x86_64::VirtAddr::new(0x4000_0000_0000);
        let frame = memory::allocate_frame().expect("smoke test: no frame");
        let phys = frame.start_address();
        let flags = PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::NO_EXECUTE;
        if let Err(e) = memory::map_page(test_virt, phys, flags) {
            kprintln!("ruos: map test failed: {}", e);
            hcf();
        }
        unsafe { test_virt.as_mut_ptr::<u64>().write_volatile(0xC0FFEEu64); }
        let back = unsafe { test_virt.as_ptr::<u64>().read_volatile() };
        if back != 0xC0FFEE {
            kprintln!("ruos: map test mismatch: 0x{:X}", back);
            hcf();
        }
        memory::unmap_page(test_virt).expect("smoke test unmap");
        memory::free_frame(frame);
        kprintln!(
            "ruos: map test ok virt=0x{:X} phys=0x{:X}",
            test_virt.as_u64(),
            phys.as_u64(),
        );
    }
```

- [ ] **Step 4: Build and run**

```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -15'
```
Expected serial inserts two new lines:
```
ruos: frames total=N used=M free=K
ruos: paging up
ruos: map test ok virt=0x4000000000 phys=0x...
ruos: lapic calibrated ...
ruos: ticks=N
```
and `TEST_PASS`. The smoke test maps PML4[128] (lower half, never touched by Limine), creating fresh PDPT/PD/PT pages via the frame allocator. If `ruos: map test failed: ...` appears, the message names the underlying error variant.

- [ ] **Step 5: Changelog**

Create `CHANGELOG/35-26-05-28-mapper-api.md`:

```markdown
# 35 — Mapper API (`OffsetPageTable`) + smoke test boot

**Data:** 2026-05-28

## Cosa
- `kernel/src/memory/mapper.rs`: wrapper di `x86_64::OffsetPageTable` con
  HHDM offset; helper `init(hhdm)`, `map_page`, `unmap_page`, `map_io_page`.
- Errori tipizzati `MapError`/`UnmapError` con `Display` (proiettano
  `MapToError`/`UnmapError` del crate `x86_64`).
- `memory/mod.rs` re-export.
- `kmain`: chiama `init_mapper`, logga `ruos: paging up`, esegue smoke test
  map/unmap su `0x4000000000` (PML4[128] fresco), logga `ruos: map test ok`.
- `apic/mmio.rs` ancora attivo per LAPIC/IOAPIC; Task 3 lo rifattora.

## Perché
Secondo pezzo dello Step 6: API paging unificata su trait `x86_64::Mapper`
con frame allocator come consumer.

## File toccati
- kernel/src/memory/mapper.rs (nuovo)
- kernel/src/memory/mod.rs
- kernel/src/main.rs
- CHANGELOG/35-26-05-28-mapper-api.md
```

- [ ] **Step 6: Commit**

```bash
git add kernel/src/memory/mapper.rs kernel/src/memory/mod.rs kernel/src/main.rs \
        CHANGELOG/35-26-05-28-mapper-api.md
git commit -m "feat(rust): OffsetPageTable Mapper wrapper + boot smoke test"
```

---

## Task 3: Refactor `apic/mmio.rs` → `memory::map_io_page`

**Files:**
- Modify: `kernel/src/apic/lapic.rs`
- Modify: `kernel/src/apic/ioapic.rs`
- Modify: `kernel/src/apic/mod.rs`
- Delete: `kernel/src/apic/mmio.rs`
- Modify: `kernel/src/main.rs`
- Create: `CHANGELOG/36-26-05-28-mmio-refactor.md`

- [ ] **Step 1: Refactor `kernel/src/apic/lapic.rs::init`**

Replace the existing body of `init` with the version below. The signature loses the `hhdm_offset` parameter — the new `memory::map_io_page` carries the HHDM offset internally.

```rust
pub fn init(phys_base: u64, spurious_vector: u8) {
    let virt = crate::memory::map_io_page(x86_64::PhysAddr::new(phys_base))
        .expect("lapic mmio map");
    // SAFETY: single-threaded boot, no other writers to LAPIC_VIRT.
    unsafe {
        LAPIC_VIRT = virt.as_u64();
        // Enable LAPIC: set bit 8 in SVR, OR in the spurious vector.
        let cur = read_volatile(reg(REG_SVR));
        write_volatile(reg(REG_SVR), cur | (1 << 8) | spurious_vector as u32);
        // Divide config = 16.
        write_volatile(reg(REG_TIMER_DIV), 0x3);
        // Mask the timer until configured.
        write_volatile(reg(REG_LVT_TIMER), TIMER_MASKED);
    }
}
```

Drop the `crate::apic::mmio::map_mmio_page(phys_base, hhdm_offset);` line.
Drop the `hhdm_offset` parameter from the signature. Do NOT change other
items in the file.

- [ ] **Step 2: Refactor `kernel/src/apic/ioapic.rs::init`**

Replace the existing body of `init`:

```rust
pub fn init(phys_base: u64) {
    let virt = crate::memory::map_io_page(x86_64::PhysAddr::new(phys_base))
        .expect("ioapic mmio map");
    // SAFETY: single-threaded boot.
    unsafe { IOAPIC_VIRT = virt.as_u64(); }

    // Read max redirection entry from IOAPICVER (index 0x01, bits 16..23).
    let ver = read(0x01);
    let max_redir = ((ver >> 16) & 0xFF) as u32;

    // Mask everything until explicit redirect() calls.
    for i in 0..=max_redir {
        let idx = REG_IOREDTBL_BASE + i * 2;
        write(idx, 1 << 16);
        write(idx + 1, 0);
    }
}
```

Drop the `hhdm_offset` parameter and the `crate::apic::mmio::map_mmio_page(...)` call.

- [ ] **Step 3: Remove the `mmio` submodule**

Edit `kernel/src/apic/mod.rs`. Replace its content with:

```rust
pub mod lapic;
pub mod ioapic;
```

(The `pub mod mmio;` line is dropped.)

Delete the file:

```bash
git rm kernel/src/apic/mmio.rs
```

- [ ] **Step 4: Update `kmain` callers**

Edit `kernel/src/main.rs`. The block that currently reads:

```rust
    apic::lapic::init(acpi_info.lapic_base, acpi_info.hhdm_offset, idt::VEC_SPURIOUS);
    apic::ioapic::init(acpi_info.ioapic_base, acpi_info.hhdm_offset);
```

becomes:

```rust
    apic::lapic::init(acpi_info.lapic_base, idt::VEC_SPURIOUS);
    apic::ioapic::init(acpi_info.ioapic_base);
```

(`hhdm_offset` is dropped from both calls.)

- [ ] **Step 5: Build and run**

```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -12'
```
Expected: full serial sequence unchanged from Task 2, ending with
`ruos: ticks=N` and `TEST_PASS`. The LAPIC/IOAPIC bring-up now flows through
`memory::map_io_page` instead of the deleted `apic/mmio.rs`; any failure
would manifest as `lapic mmio map` / `ioapic mmio map` panics or a triple
fault before the calibration line.

- [ ] **Step 6: Changelog**

Create `CHANGELOG/36-26-05-28-mmio-refactor.md`:

```markdown
# 36 — Refactor `apic/mmio.rs` → `memory::map_io_page`

**Data:** 2026-05-28

## Cosa
- `apic/lapic.rs::init` e `apic/ioapic.rs::init` perdono il parametro
  `hhdm_offset`; entrambi chiamano `crate::memory::map_io_page(phys)` per
  ottenere il virt UC e procedono col loro setup MMIO.
- Cancellato `kernel/src/apic/mmio.rs` (page-walk manuale + `Box::leak` PT
  pages + guardia `HUGE_PAGE` ad-hoc + counter `LEAKED`). Semantica
  equivalente ora vive nel typed `OffsetPageTable` di Task 2.
- `kmain` aggiornato: passa solo `phys_base` + `spurious_vector` a
  `lapic::init`, solo `phys_base` a `ioapic::init`.

## Perché
Terzo pezzo dello Step 6: una sola API paging in tutto il kernel.

## File toccati
- kernel/src/apic/lapic.rs
- kernel/src/apic/ioapic.rs
- kernel/src/apic/mod.rs
- kernel/src/apic/mmio.rs (rimosso)
- kernel/src/main.rs
- CHANGELOG/36-26-05-28-mmio-refactor.md
```

- [ ] **Step 7: Commit**

```bash
git add kernel/src/apic/lapic.rs kernel/src/apic/ioapic.rs kernel/src/apic/mod.rs \
        kernel/src/main.rs CHANGELOG/36-26-05-28-mmio-refactor.md
git add -u kernel/src/apic/mmio.rs
git commit -m "refactor(rust): route MMIO mapping through memory::map_io_page"
```

---

## Notes for the implementer

- **WSL + cargo env** for every build/run. The pinned nightly is
  `nightly-2026-05-26`.
- **Adapt to crate APIs, do not redesign.** If `x86_64` 0.15.x exposes
  `MapToError`/`UnmapError` variants under different names, project them onto
  the local `MapError`/`UnmapError` and keep the `Display` strings stable.
- **The MMIO huge-page hazard** that the deleted `apic/mmio.rs` panicked on is
  now reported by the typed Mapper as `MapError::ParentHugePage` — the same
  failure mode, surfaced more cleanly. On QEMU/VBox today, MMIO at
  `0xFEE00000` / `0xFEC00000` lives in PDPT entries Limine leaves absent, so
  the map succeeds; if a future Limine starts huge-page-mapping the relevant
  HHDM range, the panic in `lapic::init` / `ioapic::init` will say
  `lapic mmio map` / `ioapic mmio map`, and the fix is to allocate a separate
  non-HHDM virtual range for MMIO.
- **TEST_PASS is preserved at every task.** After Task 1: existing path still
  works (mmio.rs active). After Task 2: same plus the new smoke-test line.
  After Task 3: same again, MMIO via the new Mapper.
