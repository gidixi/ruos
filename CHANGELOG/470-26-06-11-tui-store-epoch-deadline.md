# 470 — Fix post-merge: epoch deadline sullo store di run_tui_component

**Data:** 2026-06-11

## Cosa

Dopo il merge di main (epoch watchdog, `epoch_interruption(true)` engine-wide)
nel branch wifi (rtop Component Model), `run_tui_component` creava lo Store
senza `set_epoch_deadline` → deadline default 0 → `wasm trap: interrupt`
immediato su ogni app TUI (run-test rosso: TEST_FAIL_RTOP, "tui-app trap").
Aggiunto `store.set_epoch_deadline(NO_DEADLINE_TICKS)` dopo `Store::new`,
stesso precedente degli altri store componenti. La policy watchdog vera per
questo runner (epoch_deadline_callback + is_kill_pending/VINTR) resta SP1
(spec multi-tenant-hardening, punto 5 — CHANGELOG 467).

run-test post-fix: TEST_PASS.

## Perché

Conflitto semantico non visibile a git: main ha reso obbligatorio il deadline
per ogni Store, il runner TUI è nato sul branch prima di quella regola.

## File toccati

- kernel/src/wasm/wt/component.rs
