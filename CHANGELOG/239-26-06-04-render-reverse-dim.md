# 239 — render reverse + dim attributes

**Data:** 2026-06-04

## Cosa
Aggiunto rendering degli attributi REVERSE (scambia fg/bg) e DIM (scurisce fg
verso bg al 63%) nella funzione `compose_cell` del render engine.

## Perché
Task 2 del Piano 2 (terminal-engine fidelity): i terminali reali supportano
REVERSE e DIM come attributi SGR standard; senza di essi la resa visiva dei
programmi che li usano (es. htop, less, vim) è incorretta.

## File toccati
- kernel/src/console/render.rs
- kernel/src/console/engine_test.rs
