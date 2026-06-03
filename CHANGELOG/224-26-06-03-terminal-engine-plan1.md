# 224 — Piano implementazione Plan 1 (foundation motore terminale)

**Data:** 2026-06-03

## Cosa
Piano d'implementazione dettagliato (TDD task-by-task) per il Plan 1 del motore
terminale: refactor della console framebuffer su architettura back-buffer ibrido
(Grid celle + pixel back-buffer Surface + GlyphCache maschere alpha + render
dirty-blit), API pubblica invariata, blink cursore IRQ invariato. 9 task con
self-test in-kernel asseriti via nuovo target `make run-console-test`.

## Perché
Tradurre lo spec approvato (CHANGELOG 223) in passi eseguibili. Plan 1 copre la
parte "veloce" + il refactor architetturale; truecolor/attributi/alt-screen ecc.
restano a Plan 2 (fidelity) e Plan 3 (modern VT), che dipendono dai tipi del
Plan 1.

## File toccati
- docs/superpowers/plans/2026-06-03-terminal-engine-foundation.md
- CHANGELOG/224-26-06-03-terminal-engine-plan1.md
