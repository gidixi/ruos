# 30 — Review fix Step 5 (IDT/APIC milestone)

**Data:** 2026-05-28

## Cosa

Correzioni applicate dalle code review nei vari task dello Step 5:

- **Task 2:** `kernel/src/kprint.rs` ora avvolge il corpo del macro
  `kprintln!` in `x86_64::instructions::interrupts::without_interrupts(...)`.
  Previene deadlock se un ISR (timer, tastiera) chiama `kprintln!` mentre
  il main thread tiene `SERIAL.lock()`.
- **Task 3:** `kernel/src/acpi_init.rs` usa `checked_sub` + variante
  `RsdpBelowHhdm` (al posto di `saturating_sub` che azzerava silenziosamente
  un indirizzo non-HHDM). Aggiunto commento sul lifecycle di `acpi_info` in
  `kmain` (consumato dal Task 4).
- **Task 4:** `kernel/src/apic/mmio.rs` aggiunta guardia `HUGE_PAGE`
  (`PageTableFlags::HUGE_PAGE`) in `next_table_or_create`: se incontra un
  entry presente con PS=1 (1 GiB o 2 MiB leaf), panica anziché trattare un
  data frame come tabella e corrompere RAM. Stesso pattern del fix C in
  `edb02d3`.
- **Task 4:** `kernel/src/apic/ioapic.rs::redirect` riordinata in
  mask → write high → atomic low (vector+unmask): evita interrupt
  spuri con destination/vector mismatched durante l'aggiornamento di un
  entry vivo.
- **Task 4:** `kernel/src/timer.rs::init` widening a `u64` per la
  moltiplicazione `lapic_per_10ms * 100` (evita overflow per LAPIC >4.29 GHz).
  Aggiunto check `> u32::MAX` con error `hz too low`.

## Perché

Chiudere i rilievi delle review prima di considerare lo Step 5 completo.
Tutti i fix preservano TEST_PASS (`ruos: ticks=N`).

## File toccati

- kernel/src/kprint.rs (Task 2)
- kernel/src/acpi_init.rs, kernel/src/main.rs (Task 3)
- kernel/src/apic/mmio.rs, kernel/src/apic/ioapic.rs, kernel/src/timer.rs (Task 4)
- CHANGELOG/30-26-05-28-step5-review-fixes.md
