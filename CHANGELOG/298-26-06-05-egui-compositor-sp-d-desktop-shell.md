# 298 — egui SP-D: Compositor IS the Desktop (userspace desktop shell, verified)

**Data:** 2026-06-05

## Cosa

SP-D completo — il COMPOSITOR È IL DESKTOP (Model A). Tutti i componenti implementati,
buildati, verificati headless (boot-check) e visualmente (QEMU screenshot).

**gui-core `shell_chrome`** (pure intents, in `ruos-desktop/gui-core/`):
- Top panel: pulsante "☰ Apps" (apre launcher), clock (wall_seconds), ⏻ poweroff.
- Wallpaper sfumato e sfondo desktop.
- Il path `Desktop`/`gui.cwasm` è rimasto invariato (nessuna regressione).

**Crate `shell`** (wasip1 reactor `ruos-window`, in `ruos-desktop/shell/`):
- Primo frame: `wm.set_background()` → si auto-dichiara fullscreen bg + `wm.surface_size()` → riempie tutto lo schermo.
- `frame_once_bare` (nessun titlebar).
- Click sul launcher → `ruos_window::spawn(id)`.
- ⏻ → `wm.poweroff()`.
- CATALOG con le app: egui-demo, About, Files, Terminal, System Monitor.

**ruos-window** — nuovi wrapper: `frame_once_bare`, `poweroff`, `surface_size`, `wall_seconds`.

**Kernel host fns** (in `kernel/src/wasm/wt/wm.rs`):
- `wm.poweroff` → `power::poweroff()`.
- `wm.surface_size` → `gfx::geom()` packed come `(w<<32)|h` (i64).
- `Compositor::new` boota `"shell"` (fallback a egui-demo se VFS non montato).

**Makefile + limine.conf**: builda e shippa `shell.cwasm` in `/bin`; limine.conf lo monta.

**Boot-check fix** (incluso in questo PR):
- `spd_self_test()` restituiva `(w<<16)|h` (u32, troncamento 16-bit) mentre il runtime
  `wm.surface_size` usa `(w<<32)|h` (i64). Uniformati: return type → `i64`, packing →
  `((sw as i64) << 32) | (sh as i64)`, unpack in `interrupts.rs` → `spd >> 32` /
  `spd & 0xffff_ffff`. Commenti aggiornati di conseguenza.

**Verifiche:**
- Boot-check `TEST_BOOT_PASS` con `spd: hostfns ok bg=1280x800` (corretto).
- QEMU log: `spawn app='shell'` + `bg window` + `wm.spawn ok name='egui-demo'`.
- Screendump desktop: wallpaper + panel + menu Apps + finestra lanciata visibili.
- VBox: clean boot, nessuna regressione.
- `gui.cwasm` (percorso `Desktop`) continua a buildare e girare invariato.

Le app del launcher (About/Files/Terminal/System Monitor) sono catalogate ma i loro
`.cwasm` arrivano in SP-E (wm.spawn è un no-op fino ad allora).

Riferimento spec/piano: `docs/superpowers/specs/2026-06-05-egui-compositor-sp-d-spec.md`
e il piano di implementazione SP-D.

## Perché

SP-D è il pivot che trasforma il compositor da gestore-di-finestre passivo in
DESKTOP ATTIVO: la shell WASM si auto-dichiara background fullscreen, rende il
panel/launcher, e può lanciare altre app WASM — tutto senza codice kernel aggiuntivo,
solo host fn già esistenti + 2 nuove (`poweroff`, `surface_size`).

Il fix al packing elimina un'inconsistenza cosmetic tra il diagnostico boot-check e
il runtime: con 1280 che sta in 16 bit il valore era numericamente lo stesso, ma il
tipo e la semantica erano sbagliati.

## File toccati

- `kernel/src/wasm/wt/wm.rs` — host fn `wm.poweroff`/`wm.surface_size`, `Compositor::new` boot `"shell"`, `spd_self_test` fix (i64, w<<32)
- `kernel/src/wasm/wt/mod.rs` — `run_spd_demo()` return type i64, commento aggiornato
- `kernel/src/boot/phases/interrupts.rs` — unpack spd `>>32` / `&0xffff_ffff`, commento aggiornato
- `Makefile` — build + ship `shell.cwasm` in `/bin`
- `limine.conf` — monta shell.cwasm
- `ruos-desktop/ruos-window/src/lib.rs` — `frame_once_bare`, `poweroff`, `surface_size`, `wall_seconds`
- `ruos-desktop/gui-core/src/shell.rs` — `shell_chrome` (panel + launcher + clock + poweroff + wallpaper)
- `ruos-desktop/shell/src/main.rs` — crate shell wasip1 reactor (bg desktop + launcher → spawn)
- `build/spd_verify.py` — helper verifica marker boot-check
