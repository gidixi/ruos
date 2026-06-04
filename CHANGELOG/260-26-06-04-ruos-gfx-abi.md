# 260 — ruos_gfx ABI + framebuffer GUI service (#4)

**Data:** 2026-06-04

## Cosa
Host function grafiche per una GUI `.cwasm` (Wasmtime) + servizio framebuffer
kernel, allineati all'abi del repo PC `ruos-desktop/gui-core/src/abi.rs` (la GUI
si sviluppa/testa su PC con winit, poi si porta in ruos):
- `kernel/src/gfx/mod.rs`: `GUI_MODE`; `geom()` (GfxInfo 4×u32, **format
  RGBA8888 = 0**); `blit()` RGBA→layout (RGB/BGR) con clipping; coda eventi GUI
  (wire 16B `[kind,p0,p1,p2]`); `push_key` (scancode grezzi, estesi 0xE0NN);
  `fold_mouse` (delta PS/2 → posizione **assoluta f32** + edge bottoni); enter/
  leave (sospende/ripristina console). Geometria reale catturata da `gfx::init`
  in fase devices (statics propri, immuni al clobber di `engine_test`).
- `kernel/src/wasm/wt/gfx.rs`: `gfx_info`/`gfx_blit`/`gfx_poll_event` sul
  `Linker<WtState>`; `gfx_info` entra in GUI mode; `run_cwasm` linka gfx e fa
  `leave()` a fine.
- Console (`fb.rs`): `write_str`/`tick_cursor` saltano il paint in GUI mode.
- Tastiera: in GUI mode i scancode vanno alla coda gfx (no PTY).

Verificato in QEMU (boot-checks): `geom 1024x768`; gfx blit self-test ok
(readback pixel rosso); gfx host self-test ok (gfxtest.cwasm chiama
gfx_info+gfx_blit via Linker). E2E con la GUI vera = quando si porta gui-core.

## Perché
Prerequisito #4 del desktop egui. Le ABI qui DEVONO restare identiche al crate
`abi` del repo PC. Wall-clock per Platform e gfx_poll_event bloccante (epoch)
= follow-up; la GUI può usare poll-based.

## File toccati
- kernel/src/gfx/mod.rs, kernel/src/main.rs
- kernel/src/console/fb.rs, kernel/src/keyboard/mod.rs
- kernel/src/boot/phases/{devices.rs,fs.rs}
- kernel/src/wasm/wt/{gfx.rs,mod.rs}, kernel/src/wasm/wt/gfxtest.cwasm
- tools/wt-gfxtest/gfx.wat
