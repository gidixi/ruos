# 532 — repaint: cap dei soli mouse-move (fix lag menu su HW reale)

**Data:** 2026-06-15

## Cosa

In `ruos-window` `repaint_gate`: il render-on-input ora scatta SUBITO solo sugli
eventi **discreti** (click/tasto/wheel/resize/quit). I `MouseMove` da soli vengono
**coalescati** al cap (`REPAINT_CAP_DIVIDER`, ~20fps).

```rust
let has_discrete = EVENT_ACC.iter().any(|e| !matches!(e, GfxEvent::MouseMove { .. }));
if has_discrete || FRAME_TICK % REPAINT_CAP_DIVIDER == 0 { /* render */ }
```

## Perché

Su VBox il menu era fluido, su HW reale laggava muovendo il mouse SOPRA il menu.
Causa: render-on-input (changelog 526) ridisegnava lo shell ad OGNI evento mouse.
Su VBox il mouse è assoluto a rate basso → poche iterazioni con move → il loop
(che fa `hlt` incondizionato ogni giro, wm.rs:3697) resta idle. Su HW reale il
mouse USB HID arriva ad alto rate (drenato a 100Hz da `usb_poll_task`) → quasi ogni
iterazione ha un move → render pieno ogni iterazione → il render sfora il periodo
timer (10ms) → `hlt` ritorna subito → HLT→0, core saturo, highlight/cursore
scattano (era la tempesta del 517, visibile solo su HW). Cappando i soli move, 4
iterazioni su 5 saltano il render (cheap) e l'`hlt` riempie i ~10ms → idle
ripristinato; i click restano istantanei (discreti → render subito).

Fix **tutto app-side**: il path input kernel (`fold_mouse`, IRQ mouse, polling USB
sul BSP executor) è INVARIATO → eventi USB non influenzati. HW-confermato.

## File toccati

- ruos-desktop/crates/ruos-window/src/lib.rs
