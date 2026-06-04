# 246 — cursor show/hide (?25) + DECSCUSR styles

**Data:** 2026-06-04

## Cosa
- `?25l`/`?25h` (DECTCEM): nasconde/mostra il cursore via `CURSOR_VISIBLE: AtomicBool`.
- `DECSCUSR` (`CSI n SP q`): imposta lo stile del cursore — 0/1/2 → block, 3/4 → underline, 5/6 → bar.
- `tick_cursor` legge `CURSOR_VISIBLE` e ritorna subito se nascosto; legge `CURSOR_STYLE`
  e XOR la regione appropriata (block = cella intera, bar = 2 colonne, underline = 2 scanline bottom).
- Test T38-40 in `engine_test.rs` verificano hide/show/stile.
- Nota: blink/steady non distinto (sempre lampeggia) — semplificazione intenzionale.

## Perché
Task 2 del Piano 3 (terminal-engine modern VT): supporto cursor visibility e stili
DECSCUSR come richiesto da app TUI (vim, nano, ratatui, etc.) che nascondono/cambiano
il cursore durante il rendering.

## File toccati
- kernel/src/console/fb.rs
- kernel/src/console/engine_test.rs
