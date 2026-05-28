# 23 — Spec design: IDT/GDT + APIC + Timer + Tastiera

**Data:** 2026-05-28

## Cosa

Scritta la spec dello Step 5 in
`docs/superpowers/specs/2026-05-28-rust-idt-apic-design.md`. Architettura:
GDT + TSS (1 IST per #DF) → IDT con handler eccezioni (#DE/#UD/#GP/#PF/#DF/#BP)
→ PIC 8259 mascherato → ACPI parsing via crate `acpi` da RSDP Limine →
LAPIC + IOAPIC (codice nostro xAPIC MMIO) → LAPIC timer 100 Hz periodico →
PS/2 keyboard su IRQ1 → IOAPIC. Smoke test seriale `ruos: ticks=N`; tastiera
test manuale. Dipendenze nuove: `x86_64` + `acpi`. Decomposizione 5 task.

## Perché

Step 5 della roadmap: infrastruttura interrupt prima di Step 6 (page fault
handler nel paging Rust) e Step 7 (scheduler preemptive guidato dal timer
IRQ).

## File toccati

- docs/superpowers/specs/2026-05-28-rust-idt-apic-design.md
- CHANGELOG/23-26-05-28-idt-apic-spec.md
