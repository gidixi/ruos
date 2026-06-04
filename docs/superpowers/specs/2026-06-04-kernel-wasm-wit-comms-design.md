# Design: strato di comunicazione kernel↔wasm via WIT / Component Model

**Data:** 2026-06-04
**Stato:** approvato (approccio + staging); pronto per il piano d'implementazione.

## 1. Problema / motivazione

Il confine tra kernel ruos e le app wasm (runtime: wasmtime 45 **no_std AOT**,
`features=["runtime","custom-virtual-memory"]`, niente cranelift a runtime) oggi è
una collezione di **host function scritte a mano** registrate via
`Linker::func_wrap` in due moduli — `ruos_gfx` (6 fn) e `wasi_snapshot_preview1`
(17 fn) — con un **ABI a puntatori grezzi** (offset i32 in linear memory, packing
LE manuale via `kernel/src/wasm/wt/mem.rs`).

Conseguenze:
- Aggiungere **una** capability tocca **4 layer**: host fn kernel + `extern "C"`
  in `ruos-backend` + metodo nel trait `Platform` di gui-core + UI.
- I tipi sono **duplicati a mano** in 3 punti (es. `GfxInfo`/`GfxEvent` in
  `gui-core/src/abi.rs`, packing in `ruos-backend/main.rs`, decode in `gfx.rs`) →
  rischio di drift silenzioso.

Obiettivo: un confine **tipizzato, single-source-of-truth, wasm-native**, dove
aggiungere un servizio = editare un `.wit` + rigenerare, con i tipi **verificati
dal compilatore su entrambi i lati**.

Inventario completo della superficie attuale (23 fn) in appendice A.

## 2. Decisione

**Approccio A — Component Model pieno (WIT), realizzato a tappe.**

Confronto (dettaglio in appendice B):
- **A — Component Model**: vero wasm-native; binding host generati e
  **verificati dal compilatore** (`bindgen!`); elimina l'ABI a puntatori per il
  path GUI. Costo: toolchain pesante (guest = component), `engine_config`/hash
  `.cwasm` cambiano (precompile component), ~8-13 giorni, **ri-espone i fix egui
  attraverso un nuovo path di codegen**, kernel con **due runtime path**.
- **B — WIT come IDL su core-wasm + codec host a mano**: SCARTATO. wit-bindgen
  **non** ha un generatore host per il `Linker` core di wasmtime → il kernel
  dovrebbe scrivere a mano un codec Canonical-ABI (return-area, ownership
  list/string, `cabi_realloc` re-entrante) = la complessità di A **senza** la sua
  type-safety. Dominato.
- **C — RPC tipizzato (`ruos-abi` + postcard)**: ottimo fallback (3-4 gg, rischio
  ~zero, non tocca `engine_config`/hash), ma **non** wasm-native. Tenuto come
  stepping-stone documentato; gli enum mappano su WIT più tardi. NON scelto.

### Feasibility (gate tecnico)
- **Compila in no_std**: provato — crate `#![no_std]` con
  `wasmtime { default-features=false, features=["runtime","component-model"] }` +
  `use wasmtime::component::{Component, Linker}` **compila per
  `x86_64-unknown-none`** (build-std, 52s).
- **Da sorgente** (wasmtime 45.0.0): `component-model` tira solo
  `wasmtime-environ/component-model` + component-macro/util + `encoding_rs`
  (no_std) + `semver`; **NON** tira `std`, `async`, né `wasmtime-fiber` (solo
  `component-model-async` lo farebbe). `resource_table` usa solo `alloc`.
- **Gate residuo** (non ancora provato): che **esegua** su bare metal
  (`Component::deserialize` + instantiate + chiamata host), e che i fix egui
  reggano attraverso il codegen component. → de-rischiato allo Step 0/1.

## 3. Architettura

Un solo package `wit/ruos.wit` = sorgente unica. Un `world ruos-gui` che
**importa** interfacce tipizzate:

- `ruos:gui/gfx` — **basato su resource `surface`** (vedi §4): `get-info`,
  `surface.blit(list<u8>, x,y,w,h)` / `commit`, ecc. Il pixel buffer è
  `list<u8>` (la coppia ptr/len collassa nel Canonical ABI).
- `ruos:gui/input` — eventi **tipizzati** (`variant`/`record`) al posto del blob
  16-byte attuale; `poll-events() -> list<event>`, `pending() -> u32`.
- `ruos:gui/clock` — `wall-seconds() -> f64` (monotonico, già fixato).
- `ruos:system/power` — `poweroff()`, `reboot()`.

Tre consumatori della STESSA `.wit`:
1. **Kernel (host)**: `wasmtime::component::bindgen!` genera un trait `*Imports`
   per interfaccia; ruos lo implementa su `crate::gfx` / `crate::power`. Caricamento
   via `Component::deserialize` (AOT, stesso pattern unsafe di `Module::deserialize`)
   + `component::Linker` **sync** (niente fiber/async). `engine_config()`
   IDENTICA a oggi.
2. **Guest (`ruos-backend`)**: bindings generati (wit-bindgen) → chiama tipato
   (`gfx::surface.blit(&buf, …)`), niente più `extern "C"` + unpack manuale.
3. **gui-core**: il trait di import generato **diventa/avvolge** il seam
   `Platform`; i tipi WIT (record/variant) sono il vocabolario condiviso →
   `abi.rs` hand-mirrored sparisce. `pc-backend` implementa lo stesso trait su
   winit (gui-core usato come lib nativa su PC, niente component runtime su PC).

`wt-precompile`: nuovo flag `--component` → `precompile_component` con
`component-model` abilitato nel tool host (l'hash dei settings deve combaciare col
kernel). `gui.cwasm` resta un blob singolo; cambia solo il nome dell'API
produttore/loader.

## 4. Multi-finestra / compositor (design forward, NON in v1)

Per non doversi rompere il confine dopo, `gfx` è modellato attorno a una
**`resource surface`** fin da subito:
- **v1**: `surface.create()` ritorna **l'unica** surface fullscreen; tutto il
  rendering va lì. Comportamento identico a oggi (app fullscreen).
- **Futuro (compositor/WM)**: ogni app crea una o più `surface` (buffer
  offscreen); un **compositor** (WM kernel-side oppure app wasm privilegiata)
  possiede il framebuffer reale, compone le surface (z-order, damage, decorazioni)
  e instrada l'input alla surface a fuoco. Le `resource` del Component Model danno
  ownership/drop corretti per gli handle finestra.
- Questo è solo un **vincolo di design** sul `.wit`; il compositor è un progetto
  separato e NON è in scope qui.

## 5. Piano a tappe (incrementale, niente big-bang)

- **Step 0 — bring-up runtime (gate decisivo)**: mini component con import a sole
  **funzioni semplici** (`system.poweroff()`, `log(string)`), eseguito via un
  nuovo `run_component`, attraverso l'`engine_config` ATTUALE. Prova
  deserialize+instantiate+chiamata host su QEMU poi VBox. (Funzioni semplici, NO
  resource, per validare il runtime al minimo costo.)
- **Step 1 — egui attraverso il component**: render di testo egui via il path
  component → **ri-verifica i fix fragili** (SSE4.1/ROUNDSS, DF=0/`cld`,
  zero-init) attraverso il nuovo codegen. Screendump QEMU+VBox: testo nitido (no
  garble), cursore, orologio.
- **Step 2 — `wit/ruos.wit`**: autoring della superficie custom (`gfx` con
  `surface`, `input`, `clock`, `system/power`).
- **Step 3 — kernel host + `run_component`**: impl dei trait `bindgen!`; il lancio
  della GUI passa a `run_component`. **WASI (17 fn) + i ~50 tool wasip1 restano su
  `run_cwasm`** (path invariato).
- **Step 4 — poweroff = PRIMA capability** end-to-end sul nuovo strato (il bottone
  power del desktop), come validazione reale dello stack.
- **Step 5 — migrazione gfx/input/clock** alla `.wit`; rimozione dei vecchi
  `func_wrap` `ruos_gfx` quando `ruos-backend` è interamente su component.
- **Step 6 (dopo, opzionale)**: foldare WASI fs/stdio in `wasi:filesystem`/
  `wasi:io` con resource handle.

## 6. Cosa NON cambia

- `run_cwasm` + i ~50 tool wasip1 core-module (echo/cat/coreutils/nano/…).
- Il path wasmi più vecchio (`kernel/src/wasm/host/*.rs`) — fuori scope.
- `engine_config()` tunables (signals off, `custom-virtual-memory`, `memory_*`,
  `x86_float_abi_ok`, `detect_host_feature` sse3..sse4.2) — devono restare
  IDENTICI anche per i component (host tool incluso).
- `kernel/src/wasm/wt/platform.rs` (mmap/TLS shim) — il component model riusa lo
  stesso backend VM/codegen.

## 7. Testing

- **Boot self-test** (`boot-checks`): `run_component` su un hello-component
  embedded (analogo a `run_hello_demo`).
- **Verifica visiva egui-su-component** (QEMU+VBox screendump): regressione
  garble + cursore + orologio.
- **Unit gui-core** (raster) invariati.
- `make iso` auto-rebuild (già cablato, CHANGELOG 271) + nuovo step precompile
  component.

## 8. Rischi e mitigazioni

- **CM esegue su bare metal?** Compila no_std (provato), ma esecuzione non ancora
  provata → **gate Step 0/1** prima di committare al resto.
- **Fix egui sotto nuovo codegen**: ri-verifica a Step 1 (garble/DF/zero-init).
- **Hash `.cwasm`**: `wt-precompile` deve usare `precompile_component` + stessa
  `Config` + `component-model` nel tool host, altrimenti deserialize fallisce
  (126).
- **Pinning toolchain**: wit-bindgen / wasm-tools / (eventuale) cargo-component
  devono concordare con l'encoding component di wasmtime 45 **e** col nightly
  pinnato (`nightly-2026-05-26`). Da installare in WSL (oggi assenti).
- **Doppio runtime path** (`run_cwasm` + `run_component`): manutenzione accettata
  consapevolmente.
- **Portabilità PC**: confermare che il pattern "guest-as-lib" tenga
  `cargo run -p pc-backend` verde (gui-core linkato nativo, niente component
  runtime su PC).
- **Resource**: introdotte solo da Step 2/3 (non nel bring-up), per isolare il
  rischio.

## 9. Rimandato esplicitamente (YAGNI)

- Fold-in di WASI nel world component (Step 6, solo se serve resource per file/socket).
- Compositor / window manager completo (progetto separato; confine reso pronto).
- Component-model **async** / fiber (sync-only per ora).

---

## Appendice A — Inventario ABI attuale (path wasmtime `wt/`, 23 fn)

`ruos_gfx` (6): `gfx_info`(OUT GfxInfo 16B; side-effect `gfx::enter()`),
`gfx_blit`(IN pixel ptr/len + x,y,w,h), `gfx_poll_event`(OUT eventi 16B ciascuno),
`gfx_pending`(→count), `gfx_debug`(IN stringa→serial), `gfx_wall_secs`(→f64
monotonico).

`wasi_snapshot_preview1` (17): `proc_exit`, `fd_write`, `fd_read`, `fd_seek`,
`fd_close`, `fd_fdstat_get`, `fd_filestat_get`, `fd_prestat_get`,
`fd_prestat_dir_name`, `path_open`, `args_sizes_get`, `args_get`,
`environ_sizes_get`, `environ_get`, `clock_time_get`, `random_get`, `sched_yield`.

Note: due clock ridondanti (`gfx_wall_secs` f64 vs `clock_time_get` ns) →
consolidabili. `path_open` usa fd interi + decode bit oflags a mano = ciò che le
resource eliminano. Stub: `environ_*`, `sched_yield`, `O_DIRECTORY`→ENOTDIR
(readdir TODO). Stato in `WtState` (`kernel/src/wasm/wt/state.rs`): `fds:Vec<WtFd>`
(0/1/2=Console, 3=preopen "/", 4..=Vfs) = il modello fd-intero che le resource
sostituirebbero.

## Appendice B — Confronto approcci (sintesi)

| Asse | A: Component Model | B: WIT+codec host | C: RPC ruos-abi/postcard |
|---|---|---|---|
| wasm-native (goal) | **SÌ** | parziale | NO |
| type-safety host | forte (bindgen) | debole (codec a mano) | media (match) |
| toolchain guest | pesante (component) | bassa | nessuna |
| tocca engine_config/hash | sì | no | no |
| rischio runtime no_std | medio-alto | basso | basso |
| sforzo | ~8-13 gg | ~4-5 gg | ~3-4 gg |
| incrementale | sì (run_component a fianco) | sì | massimo |

**B dominato** (complessità di A senza la sua type-safety). **C** = fallback se il
gate runtime di A fallisse. **A** scelto perché è l'unico veramente wasm-native +
con binding host verificati dal compilatore.
