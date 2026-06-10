# 422 — Wasmtime linear memory fuori dallo heap (memory_reservation 256 MiB)

**Data:** 2026-06-10

## Cosa
`memory_reservation(256 MiB)` in `engine_config` (kernel) e `wt-precompile`
(valore hashato nel `.cwasm`, uguaglianza esatta al deserialize), più
`memory_reservation_for_growth(64 MiB)` solo kernel (runtime-only, non hashata).
Con reservation > 0 wasmtime sceglie `MmapMemory` → `wasmtime_mmap_new` →
demand paging → **frame allocator** (RAM−heap, ~15 GB su macchine reali);
prima, con reservation 0, sceglieva `MallocMemory` → `Vec::try_reserve(48 MiB)`
contigui **dallo heap talc** per ogni finestra. Corretta anche la premessa
errata della spec demand-paging 2026-06-09 (nota in testa).

## Perché
Su HW reale la 2ª app falliva con `failed to allocate 0x3000000 bytes …
memory allocation failed because the memory allocator returned an error`
(TryReserveError = heap): heap 384 MiB − 120 MiB stack AP (16 core) − 132 MiB
tmpfs bin.bgz − shell/compositor ≈ ~50 MiB liberi < 48 MiB contigui. La linear
memory era SEMPRE stata sullo heap (whack-a-mole HEAP_SIZE 16→128→256→384,
changelog 366); il demand paging del 2026-06-09 copriva solo il codice AOT.
Ora: VA-only (256 MiB nella finestra WT da 16 TiB ≈ 65k istanze/boot), frame
al touch, grow in-place entro la reservation (meno move-storm), pressione heap
azzerata per finestra → multi-app limitato dalla RAM, non dallo heap.

## File toccati
- kernel/src/wasm/wt/mod.rs
- tools/wt-precompile/src/main.rs
- docs/superpowers/specs/2026-06-09-wt-linear-mem-demand-paging-design.md
- CHANGELOG/422-26-06-10-wt-linear-mem-off-heap.md

## Note
Richiede rebuild di TUTTI i `.cwasm` (`make iso` lo fa: il precompiler cambia
→ le pattern rule rigenerano). `.cwasm` stale (es. copiati a mano su /mnt) →
errore esplicito di deserialize ("compiled with a memory reservation of 0x0"),
non silenzioso.
