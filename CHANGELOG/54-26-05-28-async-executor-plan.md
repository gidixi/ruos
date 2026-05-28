# 54 — Piano implementazione: Async executor (Step 9)

**Data:** 2026-05-28

## Cosa

Scritto il piano dello Step 9 in
`docs/superpowers/plans/2026-05-28-rust-async-executor.md`. Tre task:

1. **Executor scaffold** — dep `embassy-executor` 0.6 no-arch +
   `__pender` custom, `executor/mod.rs`, `bootstrap_task` print
   `ruos: executor up`, kmain hand-off via `executor::run()`.
   Test: HELLO → `ruos: executor up`.
2. **Delay + tick_task** — `executor/delay.rs` con `Delay(ticks)` +
   8-slot list `Mutex` + `timer_tick(now)` invocato dal
   `timer_handler`. `tick_task` loop di `Delay(100).await` + print.
   Test: HELLO → `ruos: async tick=2`.
3. **Keyboard queue + kbd_echo_task** — refactor `keyboard.rs` in
   `keyboard/mod.rs` + `keyboard/queue.rs` (ring 64 byte + Waker).
   ISR pusha invece di stampare. `kbd_echo_task` consuma.
   Test automatico invariato; verifica keyboard manuale (`make run`).

TDD kernel-style: HELLO grep sentinel cambia per task (fail-first → impl
→ pass), preservato a ogni checkpoint. Subagent escape hatch
(`NEEDS_HUMAN_VERIFICATION`) per Task 3 manual smoke.

## Perché

Tradurre lo spec Step 9 in passi eseguibili con TDD per kernel
(`make run-test` come integration test). Pronto per subagent-driven.

## File toccati

- docs/superpowers/plans/2026-05-28-rust-async-executor.md (nuovo)
- CHANGELOG/54-26-05-28-async-executor-plan.md (nuovo)
