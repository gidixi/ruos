# 294 — egui SP-B: prima finestra egui nel compositor + Client-Side Decorations

**Data:** 2026-06-05

## Cosa
Seconda tappa di "app egui reali come finestre del compositor": una **vera app
egui** (titlebar + bottone counter) gira come finestra del compositor con
**Client-Side Decorations** — l'app egui disegna TUTTA la finestra; il modulo
`decor` kernel-side (SSD) è rimosso. `gui-core` riusato invariato.

**Compositor (kernel, CSD):**
- `compose_window` = surface raw (niente banda titlebar); `Window.rect` = finestra
  intera; rimosso il disegno `decor` (titlebar/[X]/testo) + `decor::hit`.
- Host fn **`wm.start_move()`** (l'app segnala il grab della titlebar → il kernel
  guida il drag col cursore screen, riusando la `DragState`/`drag_to` di SP3 —
  Wayland-like, niente matematica coord nell'app) + **`wm.wall_seconds()`**.
- **`frame_all` reap-on-Err**: un `frame()` che ritorna Err (trap/panic=abort/
  `proc_exit`) → `close_requested` → reap (la finestra rotta sparisce pulita
  invece di congelarsi senza [X] raggiungibile).
- **Input routing posizionato** (fix emerso in verifica): inoltro mouse-move
  (hover→finestra topmost) + button-up SEMPRE (alla finestra in drag o focused) →
  il pointer egui traccia il cursore e i click/drag si completano. **Hit-rect =
  surface committata** ogni frame (CSD: la finestra È la sua surface, 480×320 ≠ i
  320×240 placeholder).
- `AppEntry.show_in_launcher`: reactor demo ritirati dal launcher (restano per i
  boot-check); `egui-demo` visibile.
- **RNG early-seed** (fix): egui semina una HashMap ahash via WASI `random_get` al
  primo frame, ma i boot-check girano prima di `userland::init()` → CSPRNG non
  seedato → panic. Seed RDRAND anticipato nel blocco boot-check + `rng::init()`
  reso idempotente.

**egui (`ruos-desktop`):** nuovo crate `compositor-app` (wasip1 reactor su
`gui-core`): `Platform` su `wm` (`present→wm.commit`, `poll_events→wm.poll_event`,
`surface_info→480×320`, `wall_clock_secs→wm.wall_seconds`), export `frame()` (un
giro egui: `ctx.run` + tessellate + `Renderer::render` + commit del buffer pieno
stride W*4); widget **titlebar CSD** riusabile (testo + [X]→`wm.close`,
drag→`wm.start_move`); app demo (label "window id N" + bottone counter).

## Verifiche
- Boot-check headless: **`egui demo spawn ok pixels=614400`** (egui istanziato
  contro `Linker<AppState>`, `_initialize`, un frame egui renderizzato + commit).
- Visual QEMU+KVM: lancio "egui-demo" → finestra con titlebar egui "egui demo" +
  [X] + "window id 2" + bottone; counter "clicked 0"→"clicked 1" (input+state);
  drag titlebar → finestra si sposta (`wm.start_move`); [X] → chiusa (reap). Testo
  nitido (fix glifo SSE4.1 tiene). VBox: boota pulito, reactor borderless rendono.
- Review (CSD kernel + input routing + RNG + crate egui): **pulita** (2 nota
  minori non bloccanti: hit-rect senza clamp al framebuffer — innocuo a 480×320;
  glifo "✕" rende come quadratino — atlas, cosmetico).

## Perché
Obiettivo nord raggiunto al primo giro: app egui vera, decorazioni disegnate da
egui (CSD), draggable/closable/focusable. Base per SP-C (app system-info).

## File toccati
- kernel/src/wasm/wt/wm.rs, mod.rs, compose.rs
- kernel/src/boot/phases/interrupts.rs, kernel/src/rng.rs
- ruos-desktop (submodule: compositor-app/, bump)
- Makefile
- build/egui_verify.py (driver QMP)
