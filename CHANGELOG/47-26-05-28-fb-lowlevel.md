# 47 — Framebuffer low-level (Limine FB + Noto + self-test)

**Data:** 2026-05-28

## Cosa
- Aggiunta dep `noto-sans-mono-bitmap = "0.3"` (risolto a 0.3.2).
- Nuovo modulo `kernel/src/console/` con:
  - `ansi.rs` — Rgb + WHITE/BLACK (palette completa arriva in Task 4).
  - `font.rs` — wrappers Noto Size 16 con fallback '?'.
  - `fb_init.rs` — Limine FramebufferRequest + `init()` ritorna
    `FramebufferConsole` o `FbInitError`.
  - `fb.rs` — `FramebufferConsole` con `new/clear/put_char/scroll_up/
    draw_glyph/pixel_write` (ASCII printable + \n/\r/\b/\t, scroll, no
    ANSI, no blink). `self_test('X')` legge pixel rendered e li confronta
    al raster atteso.
- `main.rs` — `FRAMEBUFFER_REQUEST` static + `mod console;` + smoke test
  al boot: log `fb ok WxH pitch=P bpp=B` + `fb test ok`/`fb test fail`.
  `FramebufferConsole` viene `mem::forget`ato (Task 3 lo attaccherà a
  MultiConsole).

## Perché
Primo pezzo dello Step 8: rendering framebuffer funzionante e verificabile
prima di toccare `kprintln!` e la fan-out infrastructure.

## Adattamenti rispetto al plan
- `limine` 0.6.3: `Framebuffer` espone `width`/`height`/`pitch`/`bpp`/
  `red_mask_shift`/`blue_mask_shift` come **campi pubblici** (non metodi),
  e `address()` (non `addr()`) ritorna `*mut ()`. `framebuffers()` ritorna
  `&[&Framebuffer]` (slice), quindi usiamo `.first()` invece di `.next()`.
- `noto-sans-mono-bitmap` 0.3.2: `get_raster_width` è `const fn`, quindi
  `font::glyph_width()` è marcato `const fn`.

## Output seriale osservato
```
ruos: vfs smoke ok n=3 buf=[abc]
ruos: fb ok 1280x800 pitch=5120 bpp=32
ruos: fb test ok
ruos: ticks=206
TEST_PASS
```

## File toccati
- kernel/Cargo.toml, kernel/Cargo.lock
- kernel/src/console/* (nuovi)
- kernel/src/main.rs
- CHANGELOG/47-26-05-28-fb-lowlevel.md
