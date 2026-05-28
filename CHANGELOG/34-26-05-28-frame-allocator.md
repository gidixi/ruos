# 34 — Frame allocator fisico (bitmap, da Limine memmap)

**Data:** 2026-05-28

## Cosa
- `kernel/src/memory.rs` → split in `kernel/src/memory/mod.rs` + `heap.rs`
  (contenuto invariato) + `frames.rs` (nuovo).
- `frames.rs`: bitmap su heap, sized dal massimo end address USABLE Limine
  (NON dal massimo assoluto: QEMU riporta un buco MMIO `RESERVED` a
  `0xFD00000000` di 12 GiB che farebbe esplodere lo heap), USABLE → free,
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
