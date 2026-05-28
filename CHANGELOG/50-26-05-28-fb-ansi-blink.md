# 50 — ANSI parser (vte) + cursor blink (Step 8 Task 4)

**Data:** 2026-05-28

## Cosa
- Aggiunta dep `vte = { version = "0.13", default-features = false, features = ["no_std"] }`
  (vte 0.13.1 risolto; il default include `no_std`, ma esplicitarlo +
  `default-features = false` lascia l'intent chiaro e disabilita `std`).
- `console/ansi.rs`: palette `VGA_16` + `xterm_256(idx)` + `apply_sgr`
  parser per CSI SGR (reset, 30-37/40-47/90-97/100-107, 38;5;N/48;5;N).
- `console/fb.rs`: `FramebufferConsole` integra `vte::Parser`; impl
  `vte::Perform` (print, execute per `\n`/`\r`/`\b`/`\t`, csi_dispatch per
  A/B/C/D/H/J=2/K/m). Atomics module-level per blink: `FB_VIRT`/`FB_PITCH`/
  `FB_BPP`/`FB_PIXEL_BGR`/`CURSOR_POS`/`CURSOR_SHOWN`/`BLINK_COUNTER`.
  `tick_cursor()` IRQ-safe (no lock): XOR delle ultime 2 scanline della
  cella cursore @ 4 Hz (100 Hz / BLINK_DIVIDER=25).
- `timer::timer_handler` ora chiama `console::fb::tick_cursor()` dopo
  `TICKS.fetch_add` e prima di `lapic::eoi`.
- `kmain`: ANSI smoke test che stampa `\x1b[31mERR\x1b[0m hello via ansi`
  + `ruos: ansi test ok`.

## Perché
Chiude lo Step 8: console framebuffer completa con escape codes e cursore
lampeggiante, pronta per shell (Step 11) e GUI (Step 13).

## File toccati
- kernel/Cargo.toml, kernel/Cargo.lock
- kernel/src/console/ansi.rs, kernel/src/console/fb.rs
- kernel/src/timer.rs
- kernel/src/main.rs
- CHANGELOG/50-26-05-28-fb-ansi-blink.md
