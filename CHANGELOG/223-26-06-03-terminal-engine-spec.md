# 223 — Spec motore terminale (Round 1)

**Data:** 2026-06-03

## Cosa
Design approvato per il motore terminale kernel: back-buffer ibrido (griglia
celle + pixel back-buffer) + glyph cache a maschera alpha + blit dirty +
write-combining (veloce); truecolor, attributi bold/dim/underline/reverse,
alternate screen, box-drawing, stili cursore, scroll regions, scrollback,
bracketed paste (moderno). UX shell userspace rinviata a un Round 2 separato.

## Perché
Console framebuffer attuale: nessun back-buffer (write_volatile per glifo +
blend AA ricalcolato → flicker/lento), SGR solo 16/256 colori (niente truecolor
né attributi → ratatui degradato). Obiettivo: TUI veloce e moderna su HW reale.

## File toccati
- docs/superpowers/specs/2026-06-03-terminal-engine-design.md
- CHANGELOG/223-26-06-03-terminal-engine-spec.md
