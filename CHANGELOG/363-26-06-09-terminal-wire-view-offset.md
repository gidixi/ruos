# 363 — terminal: wire view_offset — composed render path + cursor gating

**Data:** 2026-06-09

## Cosa
Aggiunto stato scroll al `Terminal` struct (`view_offset`, `prev_sb_len`,
`new_output`, `last_drawn_offset`) e cablato nella `ui()`:
- logica di follow (`advance_view`) invocata ogni frame dopo il reflow;
- path di render selezionata: `blit_dirty` (veloce, solo diff) quando a fondo
  e vista stabile; `blit_view` (repaint pieno composto) quando scrollati su o nel
  frame di transizione;
- cursore disegnato solo quando a fondo (`view_offset == 0`): in scrollback la
  posizione del cursore è priva di senso.

A runtime `view_offset` resta 0 (nessun input scroll ancora), quindi il
comportamento visibile è invariato. L'input scroll (wheel/scrollbar/indicatore)
arriva in Task 5.

## Perché
Task 4 della feature terminal-scrollback: mettere in pista l'infrastruttura di
render prima di esporre l'interazione utente.

## File toccati
- ruos-desktop/crates/gui-core/src/desktop/apps/terminal.rs
