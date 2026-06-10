# 421 — Fix di review: TLB shootdown a range + cache manifest launcher

**Data:** 2026-06-10

## Cosa

Fix richiesti dalla review del branch `fix/tlb-shootdown-storm`:

1. **wm.rs** — log telemetria one-shot a compositor-ready (ingresso in
   `Compositor::run()`): `tlb stats at compositor-ready: shootdowns=… full_flushes=…`.
2. **tlb.rs** — `stats()` non è più `#[allow(dead_code)]` (ora ha un consumer
   default-build: il wm); doc-comment aggiornato. Nota sull'hazard della coppia
   `SHOOT_ADDR`/`SHOOT_LEN`: la consistenza presuppone l'assenza del timeout-bail
   (dopo un TIMEOUT un handler ritardatario può leggere una coppia spaiata,
   hazard identico al vecchio codice single-addr).
3. **interrupts.rs** — il boot-check range (gated `boot-checks`) ora valida anche
   il flush CR3 REMOTO, sul modello del test remap Step 3d: sentinel 0xAAAAAAAA
   nella prima pagina → l'AP la legge (r1, cachea la traduzione) → `unmap_range`
   delle 64 pagine (UN shootdown > soglia → CR3-reload remoto) → remap della
   prima pagina a un frame NUOVO con 0xBBBBBBBB → l'AP rilegge (r2). ok se
   r1=0xAAAAAAAA && r2=0xBBBBBBBB. Skip con log dedicato se nessun AP ComputeApp.
   Inoltre `sd_grew == 1` → `sd_grew >= 1 && sd_grew < 64` (i contatori sono
   globali: non essere fragili a shootdown concorrenti futuri).
4. **wm.rs `scan_apps`** — invalidazione di `NAME_CACHE` (Module per lo spawn)
   per gli stem evitti o ri-probati: prima il launcher mostrava il manifest v2
   ma `wm.spawn` eseguiva ancora il Module v1 cached.
5. **mapper.rs `unmap_range`** — `Vec::with_capacity(pages.min(4096))`: range
   giganti sparsi non devono preallocare MB. **demand.rs** — i 2 commenti su
   RANGES-leaf-lock e commit_fault citano anche `set_flags_range`/`unmap_range`.
6. **memory/mod.rs** — rimossi i re-export inutilizzati `UnmapError` e
   `hhdm_offset` (tutti i call site usano `mapper::` diretto) — warning del
   verifier sistemato. Rimosso anche l'import `PhysAddr` inutilizzato nel test
   remap di interrupts.rs (warning pre-esistente nelle build boot-checks).

## Perché

Review del fix "TLB shootdown a range" (entry 420) e della cache manifest
(entry 419): telemetria osservabile in default build, prova end-to-end del path
CR3-reload remoto, coerenza launcher/spawn dopo update di un `.cwasm`, niente
preallocazioni eccessive, commenti allineati al codice, zero warning nuovi.

## File toccati

- kernel/src/wasm/wt/wm.rs
- kernel/src/memory/tlb.rs
- kernel/src/memory/mapper.rs
- kernel/src/memory/mod.rs
- kernel/src/wasm/wt/demand.rs
- kernel/src/boot/phases/interrupts.rs
