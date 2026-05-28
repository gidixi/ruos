# 57 — Keyboard async queue + kbd_echo_task (Step 9 Task 3)

**Data:** 2026-05-28

## Cosa

- `kernel/src/keyboard.rs` → `kernel/src/keyboard/mod.rs` (modulo
  directory, per ospitare `queue.rs`).
- `kernel/src/keyboard/queue.rs`: ring buffer 64 byte protetto da
  `spin::Mutex`, `push_from_isr` + `read_char()` future. Senders
  wrappano in `without_interrupts`; ISR wake fuori dal lock.
- ISR `keyboard_handler` non chiama più `kprintln!`: decodifica
  scancode Set 1 tramite lookup table statica e pusha solo nel queue.
  Key-release (bit 7 set) e tasti non mappati vengono scartati.
- `executor::kbd_echo_task` aggiunto, spawnato accanto a `tick_task`
  in `executor::run`.
- Test automatico invariato (`ruos: async tick=2` ancora HELLO);
  verifica keyboard manuale via `make run`.

## Perché

Terzo e ultimo task dello Step 9. Materializza il requisito (C) con
una *seconda* sorgente di IRQ-wake (keyboard ≠ timer), prova che il
pattern Waker funziona generalmente. Sblocca Step 11 (shell consumer
del queue) e Step 12 (PTY line discipline a monte).

Nota: il kernel non usa il crate `pc-keyboard`; la decodifica è
implementata con una lookup table statica Scancode Set 1 → ASCII.
Step 11 potrà introdurre un decoder più completo se necessario.

## File toccati

- kernel/src/keyboard.rs → kernel/src/keyboard/mod.rs (rinominato + mod)
- kernel/src/keyboard/queue.rs (nuovo)
- kernel/src/executor/mod.rs (kbd_echo_task + spawn)
- CHANGELOG/57-26-05-28-async-keyboard-queue.md (nuovo)
