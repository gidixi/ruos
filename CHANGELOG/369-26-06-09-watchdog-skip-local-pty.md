# 369 — Watchdog reap solo pair SSH

**Data:** 2026-06-09

## Cosa
`should_reap(origin, idle_exceeded)` puro; il PTY watchdog reap-a un pair idle
SOLO se `origin == Ssh`. I pair `LocalGui` (terminali GUI) non vengono mai uccisi.

## Perché
I terminali GUI locali devono dormire (compositor), non morire per inattività.
Il watchdog resta la safety-net per le sole sessioni SSH leak-ate.

## File toccati
- kernel/src/executor/mod.rs
