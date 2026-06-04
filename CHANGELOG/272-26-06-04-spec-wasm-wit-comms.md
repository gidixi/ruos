# 272 — Spec: strato di comunicazione kernel↔wasm via WIT / Component Model

**Data:** 2026-06-04

## Cosa
Design doc per sostituire le host function one-off (ABI a puntatori grezzi) con un
confine kernel↔wasm **tipizzato e wasm-native** basato su **WIT / Component
Model**. Spec in `docs/superpowers/specs/2026-06-04-kernel-wasm-wit-comms-design.md`.

## Perché
Aggiungere una capability oggi tocca 4 layer con tipi duplicati a mano (drift
silenzioso). Obiettivo: single-source-of-truth `.wit`, binding generati e
verificati dal compilatore su entrambi i lati; aggiungere un servizio = editare un
`.wit` + rigenerare.

## Contenuto chiave
- **Feasibility provata**: crate no_std con `wasmtime[runtime,component-model]`
  compila per `x86_64-unknown-none` (build-std); da sorgente `component-model` non
  tira `std`/`async`/`fiber`.
- **Decisione: Approccio A (Component Model pieno), a tappe.** B (WIT+codec host a
  mano) scartato perché dominato; C (RPC `ruos-abi`/postcard) tenuto come fallback.
- **Surface-resource-ready** dal v1 (una surface fullscreen) → multi-finestra/
  compositor futuro senza rompere il confine.
- **Staging**: Step 0 bring-up runtime → Step 1 egui-su-component (ri-verifica
  fix garble/DF/zero-init) → wit → host bindgen + `run_component` → poweroff prima
  capability → migrazione gfx/input. WASI + ~50 tool wasip1 restano su `run_cwasm`.
- Inventario ABI attuale (23 fn) + confronto approcci in appendice.

## File toccati
- docs/superpowers/specs/2026-06-04-kernel-wasm-wit-comms-design.md (nuova spec)
