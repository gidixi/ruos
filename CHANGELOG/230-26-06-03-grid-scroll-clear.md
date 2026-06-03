# 230 — grid: scroll_up reale + clear

**Data:** 2026-06-03

## Cosa
Sostituito lo stub temporaneo `scroll_up` con l'implementazione reale: sposta le
righe 1..rows in 0..rows-1 via `copy_within`, svuota l'ultima riga con celle
blank ai colori correnti, marca tutto lo schermo dirty. Aggiunto `clear`: svuota
tutte le celle, azzera il cursore a (0,0), marca tutto dirty. Aggiunte le
asserzioni T13–T17 in `engine_test.rs`.

## Perché
Task 5 del piano terminal-engine: la griglia deve saper scrollare (output che
supera l'ultima riga) e cancellare (sequenza ESC[2J). Senza questi due metodi
non è possibile implementare il parser ANSI né la shell.

## File toccati
- kernel/src/console/grid.rs
- kernel/src/console/engine_test.rs
- CHANGELOG/230-26-06-03-grid-scroll-clear.md
