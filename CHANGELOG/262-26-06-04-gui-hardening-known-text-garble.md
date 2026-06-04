# 262 — GUI hardening + noto glitch testo (cranelift/bare-metal)

**Data:** 2026-06-04

## Cosa
Indurimento del path GUI per far girare il desktop egui (ruos-backend) in ruos:
- **SSE abilitato a boot** (`boot/phases/arch.rs::enable_simd`: CR0.EM/MP,
  CR4.OSFXSR/OSXMMEXCPT, MXCSR=0x1F80; AVX opzionale gated da CPUID). Il codice
  AOT cranelift usa SSE.
- **`cld`** prima di eseguire un `.cwasm` (DF=0 per i `rep movs`).
- **mmap shim**: `wasmtime_mmap_new` azzera le mapping scrivibili (linear memory
  wasm zero-init).
- **Heap 16→128 MiB**, **stack boot 2→16 MiB** (deserialize cwasm ~10 MiB + egui).
- **Repaint on-demand** + `gfx_pending`/`gfx_debug`/`gfx_wall_secs` host fn.
- Raster gui-core: sampling **bilineare** + edge function in **f64**.

## Issue noto (NON risolto)
Il testo egui (glifi) è reso **garbled solo in ruos** (perfetto su PC e
nell'harness PC-wasmtime con codice/settaggi/talc identici). Isolato
esaustivamente: escluse font, formato pixel, MXCSR, DF, IRQ xmm-clobber, AVX,
talc, memory-growth/reservation, stack, codegen (none==linux byte-identico nei
blocchi codice). Resta una differenza sottile dell'ambiente bare-metal ruos.
Prossimo: dump+diff del buffer/atlante guest ruos vs harness per localizzare la
divergenza di computazione. Strumento diagnostico in `ruos-desktop/wt-harness`.

## File toccati
- kernel/src/boot/phases/arch.rs, kernel/src/memory/heap.rs, limine.conf
- kernel/src/wasm/wt/{mod.rs,wasi.rs,platform.rs,gfx.rs}
- (ruos-desktop: gui-core raster bilinear+f64, ruos-backend, wt-harness)
