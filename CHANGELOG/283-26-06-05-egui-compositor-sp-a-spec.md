# 283 — Spec SP-A: unificazione stato + WASI nel compositor (egui apps in finestra)

**Data:** 2026-06-05

## Cosa
Brainstorm + spec del primo sotto-progetto per far girare **app egui reali come
finestre del compositor**. Esplorazione del codice (workflow 5 reader): verdetto
**FATTIBILE**, nessuna barriera wasmtime/AOT — il render egui→pixel esiste già
(`ruos-desktop/gui-core`, retargettabile a buffer `w×h`), il compositor esiste già;
il lavoro è unirli. Decomposto in 3 SP: **SP-A** (unificazione stato + WASI nel
linker del compositor — il crux), SP-B (harness egui-reactor), SP-C (app system-info).

Spec di **SP-A** scritta + approvata (design): host fn WASI+wm **generiche su trait
accessor** (`HasWasi`/`HasWindow`), `AppState { wasi: WtState, win: WmState }` che le
implementa entrambe, compositor su `Linker<AppState>`. **Non-breaking**: il path
app-da-shell (`run_cwasm`) resta su `Linker<WtState>` (`WtState: HasWasi`). Verifica
= un guest **`wasm32-wasip1` di probe** (export `frame()`, alloc std, riempie surface,
`wm.commit`) che spawna dalla taskbar e si compone come finestra. Niente egui ancora.

## Perché
Obiettivo nord: "app reali nel compositor egui". SP-A isola e de-rischia il crux
(un `Linker<T>` è monomorfo su un tipo; un reactor egui serve sia WASI sia `wm`).

## File toccati
- docs/superpowers/specs/2026-06-05-egui-compositor-sp-a-state-unification-design.md
