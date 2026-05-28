# 37 — Review fix Step 6 (frames + mapper)

**Data:** 2026-05-28

## Cosa

Correzioni applicate dalle code review nei task dello Step 6:

- **Task 1:** `kernel/src/memory/frames.rs` — aggiunti commenti load-bearing
  sulla "phantom tail bits" invariante (`Frames::new` inizializza `used =
  total` non `chunks*64`) e sull'asimmetria di arrotondamento per heap region
  (floor/ceil per coprire qualsiasi frame toccato dall'heap, vs inward
  rounding del walk USABLE). `pub static FRAMES` ristretto a `pub(crate)`.
- **Task 2:** `kernel/src/memory/mapper.rs`:
  - `UnmapError` esteso a 4 variant (`NotInitialized`, `NotMapped`,
    `ParentHugePage`, `InvalidFrame`) con `Display`. Match esaustivo contro
    `x86_64::UnmapError`: future variant aggiunte rompono compile, non
    silenziosamente classate `NotMapped`.
  - `init` reso idempotente (early-return se `HHDM_OFFSET` già settato) per
    evitare split-brain in caso di doppia chiamata.
  - Commento `LOCK ORDER: MAPPER → FRAMES` documentato in `map_page`.
- **Step 6 finale:** `SAFETY` comment in `mapper::init` riscritto al
  presente, senza riferimenti a "Task 3 retires apic/mmio.rs" (Task 3 ha
  effettivamente cancellato il file: Mapper è ora sole writer PML4).

## Perché

Chiudere i rilievi delle review prima di considerare lo Step 6 completo.
TEST_PASS preservato (`ruos: ticks=10`).

## File toccati

- kernel/src/memory/frames.rs (Task 1)
- kernel/src/memory/mapper.rs (Task 2 + finale)
- CHANGELOG/37-26-05-28-step6-review-fixes.md
