# 420 — TLB shootdown a range (batch) per publish/teardown Wasmtime

**Data:** 2026-06-10

## Cosa

- `tlb.rs`: nuovo `shootdown_range(virt, pages)` — pubblica `(addr, len)` via
  atomics (`SHOOT_LEN` accanto a `SHOOT_ADDR`), UN solo broadcast IPI per
  l'intero range invece di uno per pagina. `shootdown(virt)` resta come wrapper
  single-page. Handler `on_ipi()`: `len <= 32` → loop `invlpg`; oltre → full
  flush con reload CR3 (sicuro: nessuna mappatura kernel usa
  `PageTableFlags::GLOBAL`, verificato via grep — il reload flusha quindi tutte
  le pagine della finestra WT). Telemetria: contatori `SHOOTDOWNS` /
  `FULL_FLUSHES` esposti da `tlb::stats()`.
- `mapper.rs`: nuove API range `set_flags_range` (UNA acquisizione MAPPER,
  skip not-mapped, invlpg locale per pagina modificata, UN solo shootdown
  finale se almeno una pagina era present; ritorna il conteggio) e
  `unmap_range` (idem + libera i frame DOPO lo shootdown, come il flusso
  single-page). `set_flags`/`unmap_page` invariati (ora dead nei build senza
  boot-checks → `#[allow(dead_code)]` documentato).
- `platform.rs`: `wasmtime_mprotect` / `wasmtime_munmap` /
  `wasmtime_mmap_remap` usano le API range al posto dei loop per-pagina —
  semantica identica (stessi flag, skip not-mapped, stessi return).
- `interrupts.rs`: nuovo boot-check (gated `boot-checks`) — mappa 64 pagine,
  `set_flags_range` deve costare UN solo shootdown (non 64), `unmap_range`
  rimuove tutte le traduzioni.

## Perché

Fix della tempesta di shootdown per-pagina al publish/teardown dei moduli AOT
(~38-40k broadcast sincroni al bring-up su macchine many-core → freeze di
minuti/ore del GUI core). Vedi spec
`docs/superpowers/specs/2026-06-10-tlb-shootdown-batch-design.md`.

## File toccati

- kernel/src/memory/tlb.rs
- kernel/src/memory/mapper.rs
- kernel/src/memory/mod.rs
- kernel/src/wasm/wt/platform.rs
- kernel/src/boot/phases/interrupts.rs
