# 284 — egui SP-A: unificazione stato + WASI nel linker del compositor

**Data:** 2026-06-05

## Cosa
Primo sotto-progetto verso "app egui reali come finestre del compositor" (piano
`2026-06-05-egui-compositor-sp-a-state-unification.md`, spec omonima). **SP-A
provato**: un guest **`wasm32-wasip1` (std)** spawna dalla taskbar e si compone
come finestra del compositor — senza egui ancora, solo dimostrando che WASI + `wm`
coesistono in un linker.

- **Trait accessor + AppState**: `HasWasi` (`state.rs`, impl per `WtState`) +
  `HasWindow` (`wm.rs`, impl per `WmState`); `AppState { wasi: WtState, win: WmState }`
  implementa entrambi (nessun rename dei due struct esistenti).
- **Host fn generiche**: `wasi::add_to_linker<T: HasWasi>` (17 closure ritipate via
  `wasi()`/`wasi_ref()`) e `wm::add_to_linker<T: HasWindow>` (5 closure via
  `win()`/`win_ref()`); `mem.rs` `read/write/write_u32` generiche su `T` (solo
  l'export `memory`). I vecchi `read_guest`/`write_guest` di `wm.rs` rimossi (DRY su
  `mem`).
- **Compositor su `Linker<AppState>`**: `Compositor`/`Window`/`spawn_app`/`new`/
  `new_empty`/spike costruiscono `Store<AppState>` e registrano WASI **poi** wm sullo
  stesso linker; accessi kernel-side ai campi finestra via `.win.`.
- **Non-breaking**: il path app-da-shell (`run_cwasm`, `Linker<WtState>`) compila
  invariato (`WtState: HasWasi`); shell e tool continuano a girare.
- **`run_initialize`**: helper che chiama `_initialize` una volta dopo l'instantiate
  (se esportato) a tutti i 3 siti di instantiate — necessario per i reactor wasip1
  std (no-op per i reactor no_std). (Il probe usa dlmalloc lazy, quindi non lo
  esporta, ma serve a SP-B per egui.)
- **Guest di probe** `tools/wt-wasip1-probe` (std, wasip1, reactor): `frame()` fa un
  alloc `Vec` (prova std), riempie 320×240 RGBA, `wm.commit`. Importa solo
  `wm.{commit,app_id,tick}` + `wasi.{proc_exit,fd_write,environ_sizes_get,environ_get}`
  — tutti già registrati. 4ª voce in `APPS`.

## Verifiche
- Boot-check headless: **`wasip1 probe spawn ok pixels=307200`** (il guest std/wasip1
  istanziato contro `Linker<AppState>`, `frame()`, alloc std, commit). Regressione:
  `launcher registry apps=4 modules_ok=4`, lifecycle `final_live=0` (reactor esistenti
  ok).
- Visual QEMU+KVM+QMP: click sul 4° bottone taskbar → finestra **"wasip1-probe"**
  (surface teal) spawnata da un guest std/wasip1; `spawn app='wasip1-probe' live=3`.
- VBox (VM `ruos`, EFI+6vCPU): build con WASI nel linker boota pulito, `composite
  cores=5`, launcher a 4 bottoni renderizza.
- Review (refactor: completezza closure/borrow/run_cwasm-invariato; nuovo:
  run_initialize/probe/boot-check): **pulite**.

## Perché
Risolve il crux (un `Linker<T>` è monomorfo su un tipo; un reactor egui serve sia
WASI sia `wm`). Base per SP-B (harness egui-reactor) e SP-C (app system-info).

## File toccati
- kernel/src/wasm/wt/state.rs, mem.rs, wasi.rs, wm.rs
- kernel/src/wasm/wt/mod.rs, kernel/src/boot/phases/interrupts.rs
- tools/wt-wasip1-probe/{Cargo.toml, src/lib.rs}
- Makefile
- build/probe_verify.py (driver QMP)
