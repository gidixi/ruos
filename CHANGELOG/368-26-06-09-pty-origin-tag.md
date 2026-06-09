# 368 — PTY origin tag (Ssh vs LocalGui)

**Data:** 2026-06-09

## Cosa
`PtyOrigin{Free,Ssh,LocalGui}` per pair; `set_origin`/`origin`; tag a term::open
(LocalGui) e al claim SSH (Ssh); reset in release.

Inoltre: pool PTY `NUM_PAIRS` 4→8 e refactor di tutti gli array per-pair al pattern
`[CONST; NUM_PAIRS]` (CLAIMED/SHUTDOWN/LAST_ACTIVITY/FOREGROUND/WINSIZE/ORIGIN…).

## Perché
Distinguere i terminali GUI locali (da non uccidere per idle) dalle sessioni SSH.
Il bump a 8 pair è sinergico con la feature app-sleep: i terminali idle ora restano
vivi (niente reap), quindi servono più pair per GUI + SSH coesistenti (4 = 3 usabili
si esaurivano subito).

## File toccati
- kernel/src/pty/mod.rs
- kernel/src/wasm/wt/term.rs
- kernel/src/ssh/sunset_io.rs
