# 306 — SMP Step 1b: magazine allocator promoted to default; prototype B retired

**Data:** 2026-06-06

## Cosa

Promozione del magazine per-core (`MagazineAlloc`) ad allocatore globale di default
del kernel, con pensionamento del prototipo B (`PerCoreTalc`).

**Modifiche kernel:**
- `kernel/Cargo.toml`: rimossi i feature flag sperimentali `alloc-magazine` e
  `alloc-percore-talc`; aggiunto `legacy-talc` come escape hatch (default = magazine;
  `--features legacy-talc` = piano talc Talck per bisect di regressioni).
- `kernel/src/memory/heap.rs`: `#[global_allocator]` = `MagazineAlloc` di default;
  branch `legacy-talc` = `Talck`; logica `init_heap` semplificata a 2 branch.
- `kernel/src/memory/mod.rs`: `pub mod alloc_magazine` sempre compilato (non più
  feature-gated); rimosso `pub mod alloc_percore_talc`.
- `kernel/src/memory/alloc_percore_talc.rs`: ELIMINATO (git rm — prototipo B ritirato
  per decisione §8 in `docs/superpowers/decisions/2026-06-05-allocator-architecture.md`).
- `kernel/src/memory/alloc_magazine.rs`: docs di produzione — module doc completa con
  design, invarianti canonical-layout e align>16 bypass, spiegazione cross-core free;
  rimosso linguaggio "THROWAWAY spike / NON di produzione"; aggiunto `const _: ()`
  assert per coerenza tra `NUM_CLASSES` e `MAX_SMALL`.

**Risultati test suite (magazine come allocatore di default):**
- `make test-boot` → `TEST_BOOT_PASS`
- `make run-test` → `TEST_FAIL_USB_KBD` (pre-esistente: pattern grep `"usb  keyboard ready"` non corrisponde al log `"usb  hid boot keyboard ready"` — confermato identico su `legacy-talc`)
- `make run-ssh-test` → `TEST_PASS_SSH`
- `make run-pipe-test` → `TEST_PASS_PIPE`
- `make run-fuel-test` → `TEST_PASS_FUEL`
- `make run-smp-test` → `TEST_PASS_SMP`
- `make run-smp2-test` → `TEST_PASS_SMP2` (speedup=2.74x su 3 core)

Il fallimento `TEST_FAIL_USB_KBD` è pre-esistente (mismatch tra il pattern di test e il
messaggio del kernel, non relativo all'allocatore) — identico su magazine e legacy-talc.

## Perché

Step 1b della roadmap SMP shared-nothing: adottare il vincitore dello spike (Magazine A)
ahead of Step 3 (executor per-core). Il magazine elimina la contesa sul lock talc globale
per le alloc piccole (size ≤ 2048 B, align ≤ 16) senza remote-free queue — ogni core
tocca solo la propria magazine, isolata via IF-mask contro gli ISR. Il fast path cpu_id
via RDTSCP (Step 1a, CHANGELOG/305) porta il costo per-alloc a ~tens di cicli.

Vedi `docs/superpowers/decisions/2026-06-05-allocator-architecture.md` §8 per la
motivazione completa della scelta.

## File toccati
- kernel/Cargo.toml
- kernel/src/memory/heap.rs
- kernel/src/memory/mod.rs
- kernel/src/memory/alloc_magazine.rs
- kernel/src/memory/alloc_percore_talc.rs (eliminato)
- CHANGELOG/306-26-06-06-smp-step1b-magazine-default.md
