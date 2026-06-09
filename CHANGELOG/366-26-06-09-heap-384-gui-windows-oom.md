# 366 — heap 384 MiB + QEMU 1024 MiB: OOM al 3° window GUI

**Data:** 2026-06-09

## Cosa

- `HEAP_SIZE` 256 → **384 MiB** (`kernel/src/memory/heap.rs`).
- QEMU `-m 512` → **`-m 1024`** su tutti i target run/test del `Makefile` (6
  occorrenze).

## Perché

Aprendo la 3ª finestra del desktop (compositor + shell + system live, poi
spawn `terminal`) `wm.spawn` falliva con:

```
spawn: instantiate failed: failed to allocate 0x3000000 bytes
(0x3000000 minimum + 0x0 memory_reservation_for_growth)
```

`0x3000000` = 48 MiB = il **minimo dichiarato** della linear memory di ogni app
egui (`--initial-memory=50331648` in `ruos-desktop/.cargo/config.toml`).
Wasmtime con `memory_reservation(0)` (`kernel/src/wasm/wt/mod.rs`) alloca
esattamente quel minimo, contiguo, per ogni istanza viva (~60 MiB/finestra con
modulo AOT + raster). 256 MiB di heap saturavano al 3° app.

Scelta: alzare l'headroom invece di tagliare `--initial-memory` (nessun cambio
di comportamento delle app, niente rischio realloc on-demand). 384 MiB fa stare
~2 finestre in più. Il bump richiede una regione USABLE ≥ 384 MiB: con `-m 512`
non è garantita, quindi RAM QEMU portata a 1024 MiB così il finder di
`init_heap` (`.find(length >= HEAP_SIZE)`) trova sempre una regione contigua.

NB: resta whack-a-mole (storia 16→128→256→384). La leva strutturale sarebbe
ridurre `--initial-memory` dell'app egui o usare il pooling allocator Wasmtime.

## File toccati
- kernel/src/memory/heap.rs
- Makefile
