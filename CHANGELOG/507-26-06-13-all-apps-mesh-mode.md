# 507 — Tutte le app finestra → mesh-mode (raster kernel-side)

**Data:** 2026-06-13

## Cosa
Convertite a mesh-mode (raster kernel-side, `enable_mesh_mode()`) le app rimanenti:
**about, files, system, notepad** (banale, finestre opache) e **notify** (overlay).

L'overlay notifiche è full-screen TRASPARENTE (alpha-blend sul desktop): il raster
kernel creava `Raster::new([0x1e,0x1e,0x1e,0xff])` (clear opaco) → avrebbe dipinto un
rettangolo scuro su tutto. Aggiunto il supporto trasparenza:
- `ruos_raster::Raster::set_clear([u8;4])` — cambia il clear + invalida canvas/diff
  (full redraw), specchia gui-core. + test `set_clear_transparent_forces_full_and_bg`.
- host fn **`wm.set_clear(rgba: i32)`** (u32 LE [r,g,b,a]; 0 = trasparente) → setta il
  clear del raster kernel della finestra. docs/api/wm.md aggiornata (27 fn).
- `ruos_window` binding `set_clear` + `WindowState::new_overlay()` chiama
  `wm::set_clear(0)` (oltre al renderer locale, per il path pixel).
- `notify-app` chiama `enable_mesh_mode()` nell'INIT.

Ora `compositor` renderizza TUTTO il desktop (shell + tutte le app + overlay) via il
raster kernel-side parallelo (`raster cores=4`); il path pixel resta nel codice ma
non più usato dalle app shipped.

## Perché
Completa la migrazione (Phase 5) al display-server kernel-side. L'overlay richiedeva
il clear trasparente propagato al raster kernel (il renderer locale è bypassato in
mesh-mode).

## File toccati
- ruos-desktop/apps/{about,files,system,notepad,notify}-app/src/lib.rs
- ruos-desktop/crates/ruos-window/src/lib.rs (binding set_clear + new_overlay)
- ruos-raster/src/lib.rs (Raster::set_clear) + src/tests.rs (test)
- kernel/src/wasm/wt/wm.rs (host fn wm.set_clear)
- docs/api/wm.md
