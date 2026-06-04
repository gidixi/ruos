# 240 — render underline attribute

**Data:** 2026-06-04

## Cosa
Aggiunto rendering dell'attributo `CellAttr::UNDERLINE` in `compose_cell`:
dopo il loop pixel del glifo, se il bit UNDERLINE è attivo si disegna una
riga orizzontale del colore `fg` effettivo (post-reverse/dim) alla penultima
riga della cella (`oy + gh - 2`).

Aggiunta asserzione T31 in `engine_test.rs`: uno spazio con UNDERLINE deve
avere pixel bianchi (fg=WHITE) a `(0, gh-2)` e `(gw/2, gh-2)`.

## Perché
Task 3 del Piano 2 (terminal-engine fidelity): il renderer deve onorare
tutti gli attributi SGR; UNDERLINE era già riconosciuto dal parser ma ignorato
in fase di compositing.

## File toccati
- kernel/src/console/render.rs
- kernel/src/console/engine_test.rs
- CHANGELOG/240-26-06-04-render-underline.md
