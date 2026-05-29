# 83 — Drop kbd_echo_task (Step 11 Task 3 / F7 Step 10.5)

**Data:** 2026-05-29

## Cosa

- Eliminato `kbd_echo_task` (definition + spawn) da
  `kernel/src/executor/mod.rs`.
- shell.wasm è ora l'unico consumer della keyboard queue (via FD 0
  → SuspendReason::KbdReadChar oppure /dev/console via ConsoleFile).

## Perché

Fix di F7 dei followup Step 10.5: race tra `kbd_echo_task` e ogni
wasm che legge stdin via fd_read(0). Con shell come unico processo
interattivo post-boot, l'eco di tastiera per debug serve a niente.

## File toccati

- kernel/src/executor/mod.rs
- CHANGELOG/83-26-05-29-shell-drop-kbd-echo.md (nuovo)
