# 208 — Timer LAPIC calibrato sul PM timer ACPI (fix lentezza HW reale)

**Data:** 2026-06-02

## Cosa
Sul **PC reale** il sistema era diventato **lento** (cursore lampeggia più piano,
tutto sluggish). Causa: dopo aver tolto il PIT dal boot (CHANGELOG 205), il timer
LAPIC veniva calibrato contro il **TSC**, la cui frequenza viene da **CPUID**. Su
quella macchina CPUID **sovrastima** il TSC → la calibrazione misura una finestra
"10 ms" troppo lunga → conta troppi tick LAPIC → periodic count troppo alto →
**timer sotto i 100 Hz** → tutto rallentato. QEMU/VBox: CPUID accurato → non si
vedeva.

## Fix
Calibrare il LAPIC contro l'**ACPI Power Management Timer** (3.579545 MHz, fisso,
presente su tutti i PC ACPI, indipendente da PIT e TSC):
- `acpi_init`: estrae la porta I/O del PM timer dal FADT (`platform.pm_timer`,
  solo SystemIO) → `AcpiInfo.pm_timer_io` + `pm_timer_32bit`.
- `lapic::calibrate(ms, pm_timer)`: se il PM timer c'è, misura la finestra di
  `ms` ms contandone i tick (preciso) e RICALIBRA anche `boot::clock` TSC dalla
  stessa finestra; altrimenti fallback al TSC (come prima).
- `timer::init`/fase interrupts passano il PM timer; log `timer 100 Hz (ref=acpi-pm|tsc)`.

Ordine di affidabilità del riferimento timer: **PM timer ACPI** (accurato, anche
con PIT morto) → TSC/CPUID (fallback). Il PIT resta fuori (hangava su UEFI).

## Perché
Il PM timer è l'unico riferimento a frequenza fissa nota affidabile quando il PIT
è gated-off (UEFI moderno) e il TSC/CPUID è impreciso. Ripristina i 100 Hz esatti.

## Limiti
- Se il PM timer è memory-mapped (raro) invece che SystemIO, si usa il fallback
  TSC (potenzialmente impreciso). La stragrande maggioranza dei PC ha PM timer
  in I/O port.
- `boot::clock::init` (kmain, pre-ACPI) resta CPUID/PIT; viene corretto dal PM
  timer nella fase interrupts (prima di USB/userland).

## File toccati
- kernel/src/acpi_init.rs (pm_timer_io/32bit in AcpiInfo)
- kernel/src/apic/lapic.rs (calibrate via PM timer + ricalibra TSC)
- kernel/src/timer.rs, kernel/src/boot/phases/interrupts.rs (passano il PM timer)
- kernel/src/boot/clock.rs (set_tsc_per_ms)

## Verifica
QEMU: TEST_PASS, `timer 100 Hz (ref=acpi-pm)`, LAPIC 63.0M ticks/sec (≈ valore
storico). Da confermare su HW reale: cursore di nuovo ~2/sec, sistema reattivo.
