# 261 — Desktop egui end-to-end in ruos

**Data:** 2026-06-04

## Cosa
Il desktop egui vero (gui-core + ruos-backend dal repo PC, compilati
wasm32-wasip1 → `gui.cwasm` ~10 MiB AOT) gira in ruos: `gui` dalla shell →
router `.cwasm` → Wasmtime → RuosPlatform (host fn ruos_gfx) → loop egui →
tiny-skia → framebuffer. Verificato headless (`GUI-DONE`, gui --frames=3) e
visivamente in VirtualBox.

Fix necessari per arrivarci:
- **SSE abilitato a boot** (`boot/phases/arch.rs::enable_sse`: CR0.EM=0/MP=1,
  CR4.OSFXSR+OSXMMEXCPT). Il codice AOT cranelift usa SSE/SSE2..SSE4.2; il
  kernel integer-only non l'aveva mai abilitato → #UD nel codice AOT.
- **Heap 16→128 MiB** (`memory/heap.rs`): deserialize del cwasm 10 MiB +
  instantiate egui + linear memory + buffer raster andavano OOM a 16 MiB.
- **WASI mancanti** (`wasm/wt/wasi.rs`): `clock_time_get`, `random_get`
  (→ crate::rng), `sched_yield`.
- **`gfx_wall_secs`** host fn (RTC) per Platform::wall_clock_secs / egui time.
- Makefile: rule `build/gui.cwasm` (builda ../../M/ruos-desktop ruos-backend +
  precompila) + stage `/bin/gui.cwasm`; limine.conf module; init di test
  `user-bin/wt-gui-init.sh` (`gui --frames=3`).

## Perché
Chiude il giro: la GUI sviluppata/testata su PC (winit) gira identica in ruos via
il backend ruos. Noto: font/glyph resi "strani" (problema di formato pixel/raster
da indagare); avvio lento (deserialize 10 MiB) — follow-up.

## File toccati
- kernel/src/boot/phases/arch.rs, kernel/src/memory/heap.rs
- kernel/src/wasm/wt/wasi.rs, kernel/src/wasm/wt/gfx.rs
- Makefile, limine.conf, user-bin/wt-gui-init.sh
- (repo PC ruos-desktop: nuovo crate ruos-backend — commit separato)
