# 28 — LAPIC + IOAPIC + timer LAPIC @ 100 Hz

**Data:** 2026-05-28

## Cosa
- `kernel/src/apic/lapic.rs`: enable SVR, EOI, configurazione timer LVT,
  calibrazione via PIT one-shot a 10 ms.
- `kernel/src/apic/ioapic.rs`: lettura IOAPICVER, mascher init di tutte le
  redirection entry, `redirect(irq, vector, overrides)` applicando IRQ
  source overrides ACPI.
- `kernel/src/apic/mmio.rs`: mini mapper per MMIO. HHDM di Limine non
  copre LAPIC/IOAPIC; `map_mmio_page` cammina il page table corrente da
  CR3+HHDM e aggiunge una mapping UC (PCD|PWT) di 4 KiB. Pagine PT
  intermedie vengono allocate dall'heap e Box::leak-ate.
- `kernel/src/timer.rs`: `TICKS: AtomicU64`, handler timer (incrementa +
  `lapic::eoi`), `init(hz)` calibra e configura timer LAPIC periodico.
- `kernel/src/idt.rs`: registra `timer::timer_handler` su `VEC_LAPIC_TIMER`
  via `idt[VEC_LAPIC_TIMER].set_handler_fn(...)` (`Index<u8>` esposto da
  `x86_64` 0.15).
- `kernel/src/acpi_init.rs`: `AcpiInfo` espone `hhdm_offset`.
- `kmain`: chiama `apic::lapic::init`, `apic::ioapic::init`, `timer::init(100)`,
  `sti`, busy-wait su `ticks() < 10`, logga `ruos: ticks=N`.
- `Makefile`: `HELLO := ruos: ticks=` — l'assertion ora prova IDT + APIC +
  EOI + timer + sti end-to-end.

## Perché
Task 4 chiude "hardware interrupts funzionanti" dello Step 5.

## Note di adattamento
- L'HHDM di Limine v11 non mappa il MMIO di LAPIC (0xFEE00000) e IOAPIC
  (0xFEC00000): la prima lettura del SVR generava un `#PF` non-presente
  su `0xFFFF8000FEE000F0`. Aggiunto `apic/mmio.rs` per estendere il page
  table esistente con mappature UC dedicate prima di toccare l'MMIO.
- `x86_64 = "0.15"` espone `Index<u8>` su `InterruptDescriptorTable`:
  registrazione handler timer via `idt[VEC_LAPIC_TIMER]`.
- Calibrazione PIT a 10 ms ha terminato senza widening (osservato
  62'320'900 LAPIC ticks/sec sotto QEMU TCG).

## File toccati
- kernel/src/apic/mod.rs, lapic.rs, ioapic.rs, mmio.rs
- kernel/src/timer.rs
- kernel/src/idt.rs
- kernel/src/acpi_init.rs
- kernel/src/main.rs
- Makefile
- CHANGELOG/28-26-05-28-apic-timer.md
