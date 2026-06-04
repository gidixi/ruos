# 247 — fix(console): erase stale cursor cell on move (ghost fix)

**Data:** 2026-06-04

## Cosa

Aggiunto tracking dell'ultima posizione del cursore (`last_cur`) in
`FramebufferConsole`. In `write_str`, prima di chiamare `render::flush`,
la cella occupata al flush precedente viene forzata dirty via
`Grid::mark_cell`. Il blit successivo la ridisegna dal back-buffer,
eliminando qualsiasi XOR residuo lasciato da `tick_cursor`.

Nuova API in `grid.rs`: `pub fn mark_cell(&mut self, col: u16, row: u16)`
— clampa silenziosamente i valori fuori range (es. dopo alt-screen swap).

Aggiunto test T41 in `engine_test.rs`: verifica che `last_cur_for_test()`
segua la nuova posizione dopo un cursor-left `\x1b[D`.

Risolve il follow-up **F1** di `docs/followups/terminal-engine.md`.

## Perché

`tick_cursor` XOR-a i pixel del cursore direttamente sul framebuffer (path
ISR, senza lock). Quando il cursore si sposta su una cella non altrimenti
dirty, il XOR residuo rimane visibile fino alla prossima scrittura su quella
cella — ghosting visibile durante la navigazione nella shell (←/→).

## File toccati

- `kernel/src/console/grid.rs` (aggiunto `mark_cell`)
- `kernel/src/console/fb.rs` (aggiunto campo `last_cur`, aggiornati
  `new`, `write_str`, `clear`; aggiunto getter `last_cur_for_test`;
  aggiornato commento in `tick_cursor`)
- `kernel/src/console/engine_test.rs` (aggiunto T41)
- `CHANGELOG/247-26-06-04-cursor-ghost-fix.md` (questo file)
- `docs/followups/terminal-engine.md` (F1 marcato risolto)
