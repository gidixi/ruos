# 24 — Piano implementazione: IDT/GDT + APIC + Timer + Tastiera

**Data:** 2026-05-28

## Cosa

Scritto il piano dello Step 5 in
`docs/superpowers/plans/2026-05-28-rust-idt-apic.md`. Cinque task:

1. GDT + TSS (IST per #DF), dep `x86_64`. Build green.
2. SERIAL globale spin-locked + macro `kprintln!` + IDT con handler
   eccezioni + smoke test `int3` (handler logga `bp ok rip=` e ritorna).
3. PIC disable + ACPI parsing via crate `acpi` (nuova `RsdpRequest`),
   esposizione `lapic_base`/`ioapic_base`/overrides.
4. LAPIC + IOAPIC (codice nostro xAPIC MMIO) + timer LAPIC 100 Hz
   calibrato via PIT. `Makefile` HELLO → `ruos: ticks=`. **TEST_PASS qui.**
5. Handler PS/2 tastiera su IRQ1 via IOAPIC redirect. Verifica manuale.

## Perché

Tradurre lo spec Step 5 in passi eseguibili e verificabili.

## File toccati

- docs/superpowers/plans/2026-05-28-rust-idt-apic.md
- CHANGELOG/24-26-05-28-idt-apic-plan.md
