# 250 — Spec: egui desktop via Wasmtime AOT

**Data:** 2026-06-04

## Cosa
Aggiunta spec di design per il comando `gui`: desktop egui completo come app
`wasm32-wasip1` eseguita da Wasmtime no_std in modalità AOT (modulo
precompilato sul build host), con backend ruos custom (tiny-skia + host fn
`ruos_gfx`). wasmi resta per gli altri tool; Wasmtime è aggiunto solo per la
GUI. Riusa `egui_demo_lib` (backend-agnostico). Include un terminale che esegue
i tool `.wasm` su PTY (host fn `ruos_proc`: `pty_open`/`proc_spawn`/`proc_poll`);
le operazioni native del desktop usano invece le host fn WASI dirette (canale
nativo) senza spawnare tool.

## Perché
L'utente vuole un desktop grafico in stile egui web demo. Valutate e scartate:
egui nel kernel (vuole std), JIT Cranelift on-device (fragile/non supportato),
Ribir (wgpu+std). AOT via Wasmtime no_std dà velocità quasi-nativa senza
portare Cranelift nel kernel — unica via perf realistica e supportata.

## File toccati
- docs/superpowers/specs/2026-06-04-egui-desktop-wasmtime-aot-design.md
- CHANGELOG/250-26-06-04-egui-desktop-wasmtime-aot-spec.md
