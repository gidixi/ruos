# 55 — Async executor scaffolding (Step 9 Task 1)

**Data:** 2026-05-28

## Cosa

- `embassy-executor` 0.6 aggiunto a `kernel/Cargo.toml`, no `arch-*`,
  features `nightly` + `task-arena-size-4096`.
- Nuovo `kernel/src/executor/mod.rs`: usa `raw::Executor` (API
  low-level) + custom `__pender` che setta una `AtomicBool`. Outer
  loop in `run()` poll → check wake-flag → `sti; hlt` (atomic via
  `interrupts::enable_and_hlt`). CPU davvero ferma in idle.
- `bootstrap_task` stampa `ruos: executor up` e parka su
  `core::future::pending`.
- `kmain` sostituisce il loop finale con `executor::run()`.
- `Makefile` HELLO → `ruos: executor up`.

## Perché

Primo dei 3 task dello Step 9. Mette in piedi l'executor con HLT
idle (= scelta esplicita "Opt 1" del brainstorm dello Step 9, contro
`arch-spin` busy che bruciava CPU). Il timer IRQ continua a fire
(cursor blink visibile) e ad ogni IRQ il loop esterno re-polla.

## File toccati

- kernel/Cargo.toml
- kernel/src/executor/mod.rs (nuovo)
- kernel/src/main.rs
- Makefile
- CHANGELOG/55-26-05-28-async-executor-scaffold.md (nuovo)
