# 244 — Piano implementazione Plan 3 (modern VT)

**Data:** 2026-06-04

## Cosa
Piano d'implementazione (TDD, 5 task) per il Plan 3 del motore terminale: le
funzioni VT "da terminale moderno" che vivono sul path console — alternate
screen buffer (`?1049h/l`), stili cursore (`DECSCUSR`) + show/hide (`?25h/l`) con
fix del ghost cursore (F1), scroll regions (`DECSTBM`).

## Perché
Completa la roadmap del motore terminale dopo Plan 1 (back-buffer) e Plan 2
(fidelity). Scrollback e bracketed paste sono esclusi: lo scrollback richiede
intercept dell'input keyboard/PTY + render a finestra (sottosistema diverso →
Plan 4); il bracketed paste è no-op senza una sorgente di paste locale → rimandato.

## File toccati
- docs/superpowers/plans/2026-06-04-terminal-engine-vt.md
- CHANGELOG/244-26-06-04-terminal-engine-plan3.md
