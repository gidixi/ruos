# 206 — rtop: render ASCII puro (niente più "?")

**Data:** 2026-06-02

## Cosa
rtop sulla console framebuffer locale mostrava "?" ovunque. Causa: ratatui
disegnava le barre Gauge col FULL BLOCK Unicode `█` (U+2588) e le tabelle col
box-drawing `─│`, ma il font della console (`noto-sans-mono-bitmap`) include
SOLO range Latin — non ha block/box → ogni glyph mancante diventa il FALLBACK
`?` (vedi `kernel/src/console/font.rs`). Su SSH renderizzava bene (font vero del
terminale), solo la console fisica era rotta.

Fix: render interattivo di rtop reso **ASCII puro**.
- Gauge per-core/memoria → `Paragraph` con barra ASCII `[####------]`
  (`ascii_bar(pct, width)`).
- Tabella processi senza bordo box (`Borders::TOP` usava `─`); l'header in
  reverse-video (ANSI, ASCII-safe) fa da separatore.
- Rimossi import `Gauge`/`Block`/`Borders`.

`--once` era già ASCII (println) — invariato.

## Perché
Funziona ovunque, anche sulla console framebuffer (font solo-Latin). Il fix
"giusto" (console che disegna i blocchi proceduralmente) è più ampio; qui si è
scelto ASCII puro per rtop.

## Limiti noti (invariati)
- CPU%/processo = 0 quando il sistema è idle (modello cooperativo single-core:
  nessun ciclo bruciato nella finestra di campionamento). Sotto carico mostra.
- MEM = dimensione linear-memory wasm; heap_used = 0 (talc non espone i byte).

## File toccati
- user/rtop/src/main.rs (ascii_bar + Paragraph al posto dei Gauge, tabella no-box)

## Verifica
`make run-rtop-test` (SSH): TEST_PASS_RTOP, 4 frame auto-refresh, `q` esce.
0 byte UTF-8 block/box nell'output (E2 9x) → niente più "?".
