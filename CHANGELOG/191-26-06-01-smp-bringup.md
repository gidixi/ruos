# 191 — SMP bring-up coordinator (Task 4)

**Data:** 2026-06-01

## Cosa
Implementato `smp::bringup()` in `kernel/src/smp.rs`:
- Legge la Limine MP response via `crate::MP_REQUEST.response()`.
- Mappa il BSP a cpu_id 0 (`set_cpu_mapping(bsp_lapic, 0)`).
- Per ogni AP non-BSP: assegna un dense cpu_id incrementale, chiama
  `set_cpu_mapping(lapic_id, id)` PRIMA di `cpu.bootstrap(ap_entry, id)`
  — garanzia che PER_CPU[id] e la LAPIC→cpu table siano pronti nel
  momento in cui l'AP parte.
- Cap a `MAX_CPUS` per mantenere gli id densi in range.
- Attesa bounded (spin ≤ 200 M iterazioni) finché tutti gli AP avviati
  chiamano `mark_online`.
- Log finale: `N/N APs online` oppure `(timeout)` oppure `no APs (1 CPU)`.
- Dichiarato `mod smp;` in `main.rs`.
- Cablato alla fine di `boot/phases/interrupts.rs::init()`, dopo STI,
  prima di `Ok(())`.

## Perché
Task 4 di SMP Fase 1: dopo i Tasks 1–3 (MP_REQUEST, idt::load(),
cpu::{set_cpu_mapping, mark_online, cpus_online, MAX_CPUS}, ap_entry),
questo coordinator completa il bring-up degli AP e li parcheggia in
`hlt` in attesa di Fase 2 (executor/scheduler).

Verificato su QEMU -smp 4: `acpi: 4 CPU(s) found`, `smp: 3/3 APs
online`, `init.sh complete`, nessun #PF/panic.

## File toccati
- kernel/src/smp.rs  (nuovo)
- kernel/src/main.rs  (aggiunto `mod smp;`)
- kernel/src/boot/phases/interrupts.rs  (chiamata `smp::bringup()`)
- CHANGELOG/191-26-06-01-smp-bringup.md  (questo file)
