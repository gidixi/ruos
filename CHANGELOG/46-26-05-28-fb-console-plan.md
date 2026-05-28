# 46 — Piano implementazione: Framebuffer console (Step 8)

**Data:** 2026-05-28

## Cosa

Scritto il piano dello Step 8 in
`docs/superpowers/plans/2026-05-28-rust-fb-console.md`. Quattro task:

1. **FB low-level** — dep `noto-sans-mono-bitmap`, Limine `FramebufferRequest`,
   `console/{ansi,font,fb_init,fb}.rs`. Render ASCII + \n/\r/\b/\t,
   scroll_up, clear. Boot self-test: render 'X' + readback + compare.
   Log `ruos: fb ok WxH ...` + `ruos: fb test ok`.
2. **Console trait + MultiConsole** — `console/{mod,serial_con}.rs`,
   global `CONSOLE: spin::Mutex<MultiConsole>` const. `kprintln!`
   refactored da `SERIAL.lock()` a `CONSOLE.lock()`. FB non ancora
   attached. Behavior immutato.
3. **Attach FB** — kmain dopo self-test stash della `FramebufferConsole`
   in `CONSOLE` via `attach_framebuffer`. Log `ruos: fb attached`. Visivo
   `make run`: boot log post-attach visibile su schermo.
4. **ANSI vte + cursor blink** — dep `vte`, palette VGA_16 + xterm_256 +
   `apply_sgr`. `FramebufferConsole` integra `vte::Parser` con
   `impl vte::Perform` (print/execute/csi_dispatch subset CSI: A/B/C/D/H,
   J=2, K, m). Timer ISR `tick_cursor()` IRQ-safe via atomics (XOR
   underline ultime 2 scanline cella @ 4 Hz). Smoke
   `\x1b[31mERR\x1b[0m hello via ansi` + `ruos: ansi test ok`.

`TEST_PASS` preservato a ogni checkpoint.

## Perché

Tradurre lo spec Step 8 in passi eseguibili e verificabili.

## File toccati

- docs/superpowers/plans/2026-05-28-rust-fb-console.md
- CHANGELOG/46-26-05-28-fb-console-plan.md
