# 289 — Doc: sync completo dei documenti di progetto

**Data:** 2026-06-05

## Cosa
Riallineati alla realtà di `main` i quattro documenti di progetto: `README.md`,
`CLAUDE.md`, `docs/ARCHITECTURE.md`, `docs/superpowers/roadmap-rust-os.md`.

Correzioni principali (trasversali):
- **GUI = egui**, non `rlvgl` (pivot): rasterizzata on-device con `tiny-skia`,
  eseguita come `.cwasm` su **Wasmtime AOT**, host fn `ruos_gfx` + bridge WIT.
- **Secondo runtime**: Wasmtime AOT no_std (runtime-only) accanto a `wasmi`.
- **Compositor / window manager kernel-side** (multi-finestra a processi WASM
  separati, focus, drag/raise/close, compositing SMP, launcher).
- **USB mouse** + tastiera/mouse USB nella GUI; fix HW reale (PED dopo reset,
  interval speed-aware, `usb::poll()` pompato dalla GUI).
- **Heap 128 MiB** (era documentato come 4 MiB in CLAUDE.md e 16 MiB in
  ARCHITECTURE.md — entrambi errati vs `HEAP_SIZE` nel codice).
- **"hobby OS" → "WASM-first OS"** nel titolo/intro README + CLAUDE: il termine
  sottostimava la portata; il "honest ceiling" (ring 0, no isolamento hardware,
  ecc.) resta invariato, solo senza l'etichetta "hobby". Corretta anche la
  sezione roadmap "rifiutato esplicitamente" (SMP ora implementato, HW reale ora
  verificato).

Per-doc:
- `README.md`: banner ASCII "ruOS" + tagline in testa; status table (step 17-19),
  "Built alongside" (Wasmtime, GUI, USB
  mouse), real-hardware, repo layout (`gfx/`, `mouse/`, `wasm/wt/`, submodule),
  build (submodule init + target wasm), test (compositor + boot-checks/usb-probe),
  security model (due runtime).
- `CLAUDE.md`: **build env corretto** (`/mnt/w/Work/GitHub/ruos`, distro
  `Ubuntu-22.04`; era `/mnt/e/...` + `Ubuntu`), nota submodule/target/git-remote,
  Stato, roadmap step 1-19 ✅.
- `docs/ARCHITECTURE.md`: thesis, layer cake, boot phases, driver USB+mouse,
  sezione runtime Wasmtime AOT, source map, e soprattutto una nuova sezione
  **"GUI: the kernel↔WASM contract and the compositor"** approfondita:
  - **comunicazione disaccoppiata kernel↔WASM**: import/export come unico canale,
    i due stili di ABI (host module raw `func_wrap` con marshalling a mano vs.
    **WIT / Component Model** generato da un `.wit` condiviso con `wit-bindgen`
    lato guest e `bindgen!` lato host — type-safe, niente layout da sincronizzare);
  - **egui**: come la UI portabile (`gui-core`) gira su PC e su ruos invariata,
    rasterizzata `tiny-skia`, via la sola ABI `gfx` (input-agnostica);
  - **compositor**: finestre process-isolated (un'istanza WASM per finestra, ABI
    `wm`), il loop per-frame completo (reap → input-routing/focus/drag/close →
    `frame()`+commit → decorazioni → compositing SMP a bande → present).
- `roadmap-rust-os.md`: North star, "Stato del codice" (snapshot 05-28 → stato
  attuale), Step 17 (egui/USB mouse/compositor), Step 16-bis (Wasmtime AOT),
  decisioni tecniche.

## Perché
I doc erano fermi a prima del lavoro GUI/egui/compositor/Wasmtime e USB mouse;
CLAUDE.md aveva path di build stantii (rischio per i lavori futuri).

## File toccati
- README.md
- CLAUDE.md
- docs/ARCHITECTURE.md
- docs/superpowers/roadmap-rust-os.md
