# 56 — Delay future + tick_task (Step 9 Task 2)

**Data:** 2026-05-28

## Cosa

- `kernel/src/executor/delay.rs`: `Delay(target_ticks)` future + list
  globale di 8 slot (`Mutex<[Option<Slot>; 8]>`). `timer_tick(now)`
  chiamata dal timer ISR via `try_lock` per evitare deadlock.
  Senders wrappano in `without_interrupts`.
- `timer::timer_handler` ora invoca `delay::timer_tick(now)` dopo
  `tick_cursor` e prima di `eoi`.
- `executor::tick_task` rimpiazza `bootstrap_task`: loop di
  `Delay::ticks(100).await` + `kprintln!("ruos: async tick={n}")`.
- `Makefile` HELLO → `ruos: async tick=2`.

## Perché

Secondo dei 3 task dello Step 9. Materializza i due requisiti chiave
del milestone: (B) un task asincrono che ritorna allo scheduler ad
ogni iterazione, (C) wake da IRQ (timer) verso il future.

## File toccati

- kernel/src/executor/delay.rs (nuovo)
- kernel/src/executor/mod.rs
- kernel/src/timer.rs
- Makefile
- CHANGELOG/56-26-05-28-async-delay-tick-task.md (nuovo)
