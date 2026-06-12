# 482 — kernel heap 384→768 MiB for the JS-enabled viewer

**Date:** 2026-06-12

## What

- `kernel/src/memory/heap.rs`: `HEAP_SIZE` 384 MiB → **768 MiB**.
- `Makefile`: every QEMU `-m 1024` → **`-m 2048`** (the heap claim needs one
  contiguous USABLE memmap region ≥ HEAP_SIZE; 1 GiB was too tight for 768 MiB).

## Why

Embedding QuickJS (rquickjs) into the viewer grew `viewer.cwasm` (Wasmtime-AOT)
to ~83 MiB. The kernel loads a `.cwasm` by reading the whole file into one
contiguous `Vec` (`wasm::read_all`) before `Module::deserialize`. With shell +
notify already live (each holding a 48 MiB linear-memory minimum + its AOT
image), the 384 MiB heap could not satisfy the 83 MiB contiguous read:

```
KERNEL PANIC: memory allocation of 83250488 bytes failed   (= viewer.cwasm size)
```

768 MiB leaves room for the 83 MiB read plus the baseline windows; QEMU at
2 GiB provides a contiguous USABLE region ≥ 768 MiB.

## Notes

- The JS-enabled `viewer.cwasm` will keep growing through the JS bridge phases
  (more bindings); 768 MiB has headroom but watch `meminfo`/`HEAP_SIZE` as
  features land. Real hardware sizing is separate (this is the QEMU dev config).
- Pairs with CHANGELOG 100 (the `wm.frame_deadline_*` host fns) — same viewer
  JS-engine work.
