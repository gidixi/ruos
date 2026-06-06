# 309 — Fix: AP LAPIC timer divider (3a follow-up)

**Data:** 2026-06-06

## Cosa
`set_timer_periodic` ora setta il Divide Configuration Register (`REG_TIMER_DIV = 0x3`,
divide-by-16) prima di programmare il count, così l'arming è self-contained.

`kernel/src/apic/lapic.rs`: aggiunta `write_volatile(reg(REG_TIMER_DIV), 0x3)` in
`set_timer_periodic`.

## Perché
Bug introdotto in 3a (CHANGELOG/308): `init_ap()` non setta il DCR del timer (solo
`init()` sul BSP lo faceva, a riga 48). Quando l'AP armava il proprio timer via
`start_ap_timer → set_timer_periodic`, il DCR restava al default → il timer AP girava
~16x troppo veloce. Boot-check rivelatore: `ap1 ticks in 50ms = 78` invece di ~5.
Dopo il fix: `ap1 ticks in 50ms = 4–5` (100 Hz, allineato al BSP). Verificato a `-smp 4`,
2 run. La scrittura sul BSP è ridondante (init già a 0x3) → nessun cambiamento BSP.

Critico per 3b: un task AP che usa `Delay` avrebbe avuto timing ~16x sbagliato.

## File toccati
- kernel/src/apic/lapic.rs
- CHANGELOG/309-26-06-06-smp-step3a-ap-timer-divider-fix.md
