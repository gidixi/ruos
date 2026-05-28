# 45 — Spec design: Framebuffer console + trait Console (Step 8)

**Data:** 2026-05-28

## Cosa

Scritta la spec dello Step 8 in
`docs/superpowers/specs/2026-05-28-rust-fb-console-design.md`. Architettura:

- Trait `Console` (write_str + clear) + `MultiConsole` global che fa fan-out
  a `SerialConsole` + `Option<FramebufferConsole>`.
- `kprintln!` refactor da `SERIAL.lock()` a `CONSOLE.lock()` (seriale resta
  passthrough raw, framebuffer aggiunge canale visivo).
- Framebuffer Limine (`FramebufferRequest`) + render font moderno
  **Noto Sans Mono Size 16** via crate `noto-sans-mono-bitmap`.
- Scroll, cursor pos tracking, white-on-black default.
- **Cursor blink** ~4 Hz via hook in `timer::timer_handler` (XOR diretto su
  FB MMIO, no lock dall'ISR — usa `AtomicPtr`/`AtomicU64` per posizione).
- **ANSI escape codes** via crate `vte` (parser battle-tested Alacritty,
  no_std+alloc). Subset implementato: cursor A/B/C/D/H, clear `2J`/`K`,
  fg/bg 16-color + 256-palette, `0m` reset. Sequenze ignote droppate.
- Decomposizione 4 task: (1) FB low-level + self-test, (2) Console trait +
  MultiConsole + refactor kprintln, (3) attach FB a MultiConsole,
  (4) ANSI vte + cursor blink.

## Perché

Step 8 della roadmap WASM-first: console grafica per il boot + base per
Step 11 (shell line editing) + Step 13 (rlvgl). Cursor + ANSI fanno parte
del milestone su richiesta esplicita.

## File toccati

- docs/superpowers/specs/2026-05-28-rust-fb-console-design.md
- CHANGELOG/45-26-05-28-fb-console-spec.md
