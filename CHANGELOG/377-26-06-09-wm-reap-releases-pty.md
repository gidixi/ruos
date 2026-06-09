# 377 — Reap finestra chiude il PTY legato

**Data:** 2026-06-09

## Cosa
Quando il compositor reap-a una finestra con un PTY legato (`wake_pty >= 0`), chiama
`pty::request_shutdown` su quel pair → la shell esce, il pair torna Free.

## Perché
Lifecycle deterministico dei pair dei terminali GUI: il watchdog non li uccide più
per idle (Task 2), quindi la chiusura della finestra deve liberare il pair.

## File toccati
- kernel/src/wasm/wt/wm.rs
