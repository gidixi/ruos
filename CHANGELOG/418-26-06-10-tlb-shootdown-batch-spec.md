# 418 — Spec: TLB shootdown batch + manifest cache (fix freeze compositor multi-core)

**Data:** 2026-06-10

## Cosa
Spec del fix per il freeze del compositor su macchine multi-core: shootdown TLB
a range (un broadcast per range invece che per pagina, full-flush CR3 oltre 32
pagine), API `set_flags_range`/`unmap_range` nel mapper, `platform.rs` Wasmtime
che le usa, cache dei manifest nel launcher (niente deserialize+instantiate+drop
ripetuto di ogni app), telemetria contatori shootdown.

## Perché
Root-cause confermata da indagine multi-agente: il publish/teardown dei moduli
AOT fa ~38-40k broadcast IPI sincroni (uno per pagina, attesa di N-1 ack
ciascuno) al bring-up del compositor — minuti/ore su 16 core (VBox/HW reale),
quasi no-op con pochi core. Refutate: busy-spin AP, clock gonfiato, costo
demand-paging, contesa heap.

## File toccati
- docs/superpowers/specs/2026-06-10-tlb-shootdown-batch-design.md
- CHANGELOG/418-26-06-10-tlb-shootdown-batch-spec.md
