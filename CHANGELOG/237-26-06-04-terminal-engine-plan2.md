# 237 — Piano implementazione Plan 2 (fidelity motore terminale)

**Data:** 2026-06-04

## Cosa
Piano d'implementazione dettagliato (TDD, 6 task) per il Plan 2 del motore
terminale: truecolor SGR (`38;2`/`48;2`), attributi testo (bold/dim/underline/
reverse), e glifi box-drawing procedurali (`U+2500–257F`, subset ratatui). Si
appoggia al back-buffer del Plan 1.

## Perché
Tradurre la parte "fidelity" dello spec approvato (CHANGELOG 223) in passi
eseguibili. Plan 1 ha reso solo i colori 16/256 e nessun attributo; ratatui usa
truecolor + bold/reverse + bordi box-drawing → ora resi correttamente.

## File toccati
- docs/superpowers/plans/2026-06-04-terminal-engine-fidelity.md
- CHANGELOG/237-26-06-04-terminal-engine-plan2.md
