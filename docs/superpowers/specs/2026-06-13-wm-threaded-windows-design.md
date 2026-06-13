# Fase 2.5 — Finestre multithread (wasm-threads nel compositor)

**Data:** 2026-06-13
**Stato:** ✅ IMPLEMENTATO (changelog 495). Gate `THREADS-WIN-OK = ok
teardown=ok` su -smp 4 e -smp 1; regressione CLI (parsum/mtstress/pthread) e
run-test verdi. SCOPERTA chiave: nel modello REACTOR wasi-threads nessuno
inizializza la struct pthread del main (`__wasi_init_tp` — nei command lo fa
`_start`): senza, la prima thread_local loopa per sempre su una thread-list a
zero. Fix: le app threaded esportano `__wasi_init_tp` (link flag) e
`run_initialize` lo chiama prima di `_initialize`. BONUS: trovato il vero
motivo del fail pre-esistente di frame-smp-test — la shell window viene uccisa
dal watchdog dentro `gui_core::raster::render` sotto QEMU TCG (>3s di
rendering, budget FRAME_DEADLINE): lentezza da emulazione, non un bug — su HW
reale è fluida.
**Prerequisito:** MT Fase 2 chiusa (changelog 486-494, fiber M:N + futex +
thread-spawn + poll_oneoff).

## Obiettivo

Una **app finestra** (modello reactor: esporta `frame()`, il kernel guida il
loop) compilata `wasm32-wasip1-threads` può usare `std::thread`/rayon al suo
interno: `frame()` resta leggera e sotto watchdog, i worker girano come fiber
sui core ComputeApp. Caso d'uso: lavoro pesante in background (decodifica,
indicizzazione, calcolo) senza mangiarsi il budget di `frame()` né congelare
la UI.

## Design

1. **Detection allo spawn** (`spawn_named`): modulo con import `env::memory`
   *shared* → finestra threaded.
2. **Linker dedicato per-finestra threaded**: stesso set del compositor
   (wasi + wm + term + sys + net) + `thread-spawn` + define di UNA
   `SharedMemory` creata dal tipo dell'import. (Il linker condiviso del
   compositor non può ospitare N memorie diverse sotto lo stesso nome.)
   Spawn è raro: il costo del linker per-spawn è irrilevante.
3. **`ThreadGroup` rifattorizzato con `GroupKind`**: i campi di esecuzione
   (module/linker/argv) diventano una enum — `Cli { Linker<WtState>, base_args }`
   (path exec_threaded, invariato nel comportamento) e
   `Window { Linker<AppState>, win_id }`. Lo scheduling (RUNQ/WAITQ/credits/
   kill-group/ps) resta type-erased e identico. `add_thread_spawn_to_linker`
   diventa generico su `T: HasWasi` (il gruppo arriva da `wasi_ref().threads`).
4. **Worker di finestra** = fiber con `Store<AppState>` fresco (WmState
   "spettatore" con l'id della finestra, niente limiter v1), instantiate dal
   linker del gruppo, entry `wasi_thread_start`. `RAYON_NUM_THREADS` iniettato
   come per i CLI. Epoch deadline worker = `NO_DEADLINE` (come Fase 2).
5. **Lifecycle**: il "main" del gruppo è la finestra stessa (non un fiber);
   `Window.group: Option<Arc<ThreadGroup>>`; `remove_at` (reap/close/watchdog
   kill) → `poisoned` + `kill_group_waiters` → i worker muoiono (runnable al
   take, parked droppati, running al prossimo park). Nessun exec_threaded:
   nessuno "attende" il gruppo.
6. **Futex da `frame()`** (contesto non-fiber: il frame job del pool non può
   parcheggiarsi): degrada a attesa cooperativa `hlt` con ricontrollo +
   rispetto del timeout — niente più busy-spin né warn. LIMITE DOCUMENTATO:
   un wait INFINITO dentro `frame()` su una condizione mai notificata blocca
   quel job oltre la portata del watchdog epoch (i check epoch sono nel codice
   wasm, non nelle host call) — regola per le app: in `frame()` non bloccare,
   usare try_lock/canali non bloccanti; il lavoro bloccante va nei worker.
7. **Gate**: `tools/mtwin` (Rust cdylib `wasm32-wasip1-threads`, ABI reactor
   minima `wm::commit`): al primo `frame()` spawna un worker che incrementa
   un contatore atomico fino a 1000; `frame()` scrive il contatore nei pixel
   committati. Boot-check `THREADS-WIN-OK`: spawn headless, drive frame,
   assert contatore = 1000 (visibilità memoria condivisa worker→frame) e
   teardown pulito al reap (kill-group). In threads-test.sh.

## Fuori scope

- Limiter memoria sui worker store (v1: solo la finestra principale).
- frame() su fiber (parcheggiabile): possibile evoluzione se il limite (6)
  morde nella pratica.
- egui/rayon tessellation nelle app del submodule (abilitabile dopo, è solo
  build config).
