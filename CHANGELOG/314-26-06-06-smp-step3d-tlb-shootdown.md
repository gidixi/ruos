# 314 — Step 3d: cross-core TLB shootdown on unmap/set_flags

**Data:** 2026-06-06

## Cosa

Implementato il meccanismo di TLB shootdown cross-core (Step 3d del migration plan SMP).
Quando un core muta la page table condivisa (`MAPPER`, unica PML4) in modo che possa
lasciare un'entry stale nella TLB di un altro core — cioè **unmap** (present→absent) o
**set_flags** (restrizione permessi, es. W→RO+X per W^X) — ora viene broadcastato un IPI
`VEC_TLB_SHOOTDOWN` (0x42) a tutti gli altri core online, e il mutatore aspetta l'ack
da tutti prima di rilasciare `MAPPER`.

**Moduli aggiunti/modificati:**
- `kernel/src/memory/tlb.rs` (NUOVO): statics `SHOOT_ADDR`/`SHOOT_ACK`/`SHOOT_NEED`,
  funzione `shootdown(virt)` (broadcast IPI + bounded ack-wait), funzione `on_ipi()`
  (invlpg + fetch_add ack). Analisi deadlock documentata inline.
- `kernel/src/memory/mod.rs`: aggiunto `pub mod tlb;`.
- `kernel/src/idt.rs`: registrato `VEC_TLB_SHOOTDOWN` → `tlb_shootdown_handler`
  (invlpg + ack + EOI). Rimossa annotazione "No handler yet".
- `kernel/src/memory/mapper.rs`: `unmap_page` e `set_flags` chiamano
  `crate::memory::tlb::shootdown(virt)` dopo il flush locale. `map_page` NO
  (commento esplicativo aggiunto: x86 non cachea not-present entries).

**Gate (no-fault remap test, boot-check Task 3):**
- `kernel/src/boot/phases/interrupts.rs`: test `#[cfg(feature="boot-checks")]`
  che mappa test_virt→frame_A, fa leggere l'AP (cachea TLB entry A), rimappa→frame_B
  con shootdown, fa rileggere l'AP. Verifica `r1=0xAAAAAAAA r2=0xBBBBBBBB
  shootdown_ok=true` — prova che lo shootdown ha invalidato l'entry stale.

**Analisi deadlock (load-bearing):** `MAPPER` resta `spin::Mutex` (IRQs enabled mentre
contested). Un core che aspetta `MAPPER.lock()` continua a servire gli IPI → ack → il
mutatore procede → rilascia MAPPER. Convertire MAPPER a IrqMutex causerebbe deadlock
(IRQs mascherati → il core aspettante non vede mai lo shootdown IPI).

## Perché

Senza TLB shootdown, un core che usa una TLB entry stale (per una pagina già unmappata
o con permessi ristretti) può leggere/scrivere memoria arbitraria → corruzione silente
della memoria. È il prerequisito hard per eseguire app WASM dinamiche su core AP
(caricano/scaricano moduli + flip W^X → mutazioni MAPPER i cui TLB stale su altri core
corromperebbero). Il gate con `r2=0xBBBBBBBB` è la prova positiva che l'entry stale
è stata invalidata.

## Gate results (entrambi i run, -smp 4)

```
run 1: remap seen by ap: r1=0xAAAAAAAA r2=0xBBBBBBBB shootdown_ok=true
run 2: remap seen by ap: r1=0xAAAAAAAA r2=0xBBBBBBBB shootdown_ok=true
```

Nessun `shootdown TIMEOUT`. Regressioni: `test-boot` → `TEST_BOOT_PASS`,
`run-smp-test` → `TEST_PASS_SMP`, `run-smp2-test` → `TEST_PASS_SMP2`,
`run-ssh-gui-test` → `TEST_PASS_SSH`.

## File toccati

- `kernel/src/memory/tlb.rs` (nuovo)
- `kernel/src/memory/mod.rs`
- `kernel/src/idt.rs`
- `kernel/src/memory/mapper.rs`
- `kernel/src/boot/phases/interrupts.rs`
- `CHANGELOG/314-26-06-06-smp-step3d-tlb-shootdown.md`
