# 275 — Desktop gfx tipizzato (WIT/wit-bindgen su core-module) + bottone poweroff

**Data:** 2026-06-05

## Cosa
Sostituito l'ABA a puntatori grezzi `ruos_gfx` (kernel↔desktop) con un confine
**tipizzato** definito in `wit/ruos-gui.wit` (sorgente unica: interfacce `gfx` +
`power`), e aggiunto il **bottone power-off** del desktop come prima capability
attraverso il nuovo strato. Plan: `docs/superpowers/plans/2026-06-04-typed-gfx-core-module.md`.

## Come (Approccio B, vedi spec Appendice C)
- **Guest** (`ruos-backend`, submodule): `wit_bindgen::generate!` in **modalità
  core-module** (NIENTE component) → chiamate tipizzate (`gfx::get_info/blit/
  poll_event/pending/wall_seconds/debug_log`, `power::poweroff`) al posto del
  blocco `extern "C"` + unpacking `u32_at`/`f32::from_bits`. Resta `wasm32-wasip1`
  std su `run_cwasm`; WASI Preview 1 invariata; nessuno switch di launch.
- **Kernel** (`kernel/src/wasm/wt/gui.rs`): piccolo **codec Canonical-ABI** —
  `func_wrap` degli import `ruos:gui/{gfx,power}` decodificati via `wt/mem.rs`
  sul Linker core esistente (nessun runtime component, nessun WASI-p2). Tipi di
  ritorno scalari/record/`option` → niente list/string ritornate → niente
  `cabi_realloc`. `poll-event -> option<gfx-event>` return-area: disc i32 @0,
  record @4. Registrato in `run_cwasm` accanto a `wasi`+`gfx`.
- **Power-off**: `gui-core` trait `Platform::poweroff` + bottone `⏻` nel pannello
  (flag → `Gui::frame` → `platform.poweroff()`); `ruos-backend` → `power::poweroff()`
  tipizzato; `pc-backend` → `process::exit(0)`.

## Verifica
- Build kernel + guest puliti; `cargo test -p gui-core` 10/10.
- **Render ri-verificato** attraverso il nuovo codec (screendump QEMU+KVM): testo
  "☰ Apps"/orologio nitidi, wallpaper, cursore — **nessuna garble** (il routing
  gfx via nuovo ABI è byte-identico).
- **Power-off end-to-end** (QEMU): cursore mosso sul `⏻`, click → la macchina si
  spegne (QMP query-status → socket chiuso, QEMU uscito via ACPI shutdown).

## Note
- Il vecchio linker `ruos_gfx` resta registrato perché il boot-check `gfxtest.cwasm`
  lo usa ancora; la GUI è interamente su `ruos:gui`. Rimozione legacy + port di
  gfxtest = follow-up.
- VBox: power::poweroff scrive anche la porta 0x4004 (VBox) — verifica manuale sul
  VM consigliata al prossimo `make iso` + reboot.
- Full Component Model (Approccio A) per il desktop resta rimandato finché non
  esiste WASI-on-component (la GUI std lo richiederebbe).
- Submodule `ruos-desktop` portato a f16121c.

## File toccati
- wit/ruos-gui.wit
- kernel/src/wasm/wt/{gui.rs (nuovo), mod.rs, gfx.rs (wall_secs pub)}
- ruos-desktop (submodule): ruos-backend/src/main.rs, gui-core/src/{platform.rs, lib.rs, app.rs, desktop/mod.rs, desktop/panel.rs}, pc-backend/src/main.rs
