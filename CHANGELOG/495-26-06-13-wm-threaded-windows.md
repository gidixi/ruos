# 495 — Fase 2.5: finestre multithread (wasm-threads nel compositor)

**Data:** 2026-06-13

## Cosa

Un'app finestra (modello reactor, esporta `frame()`) compilata
`wasm32-wasip1-threads` ora può usare `std::thread`/rayon: `frame()` resta
leggera e sotto watchdog, i worker girano come fiber sui core ComputeApp.
Spec: `docs/superpowers/specs/2026-06-13-wm-threaded-windows-design.md`.

- **`ThreadGroup` rifattorizzato con `GroupKind`** (`wt/threads.rs`):
  `Cli { Linker<WtState>, base_args }` (exec_threaded, invariato) e
  `Window { Linker<AppState>, win_id }`; `run_thread_body` dispatcha la
  costruzione Store/Instance sul kind; `add_thread_spawn_to_linker` generico
  su `T: HasWasi`. Scheduling (RUNQ/WAITQ/crediti/kill-group/ps) identico.
- **Route nello spawn del wm** (`wm.rs spawn_named`): modulo con import
  `env::memory` shared → `build_threaded_window_group`: linker DEDICATO
  (wasi+wm+term+sys+net+thread-spawn) con la SUA SharedMemory (il linker
  condiviso non può ospitare N memorie sotto lo stesso nome) +
  `RAYON_NUM_THREADS`; `Window.group` + kill-group in `remove_at` (chiudere
  la finestra uccide i suoi thread — `kill_window_group`). Worker = fiber con
  `AppState` spettatore (`worker_app_state`), entry `wasi_thread_start`,
  `NO_DEADLINE`. Il probe `manifest()` del launcher ora prova anche i moduli
  threaded via linker throwaway (rimosso lo skip del changelog 492).
- **SCOPERTA `__wasi_init_tp`**: nel modello REACTOR nessuno inizializza la
  struct pthread del main (nei command lo fa `_start`) — la prima
  thread_local/pthread_key cammina una thread-list a ZERO e loopa per sempre
  (3 s di wasm puro → watchdog kill). Fix: le app threaded esportano
  `__wasi_init_tp` (link flag `-C link-arg=--export=__wasi_init_tp`) e
  `run_initialize` lo chiama una volta prima di `_initialize`. In più la
  deadline epoch va armata PRIMA dell'instantiate dei moduli threaded (hanno
  una start function `__wasm_init_memory` — gotcha changelog 470) sia in
  `spawn_named` sia nel probe.
- **Futex da `frame()`** (job del pool, non parcheggiabile): degradazione a
  attesa hlt cooperativa con rispetto del timeout (prima: busy-spin + warn).
  LIMITE documentato: wait infinito in frame() = fuori portata del watchdog —
  regola per le app: non bloccare in frame(), il lavoro bloccante va nei worker.
- **Gate `THREADS-WIN-OK`**: `tools/mtwin/` (cdylib wasm32-wasip1-threads,
  ABI reactor minima) — al primo frame() spawna un worker che conta fino a
  1001 (con una sleep: poll_oneoff su fiber finestra); frame() pubblica il
  contatore nei pixel committati; il gate kernel (`threaded_window_self_test`)
  drive frame finché il contatore arriva, poi close → assert live==0
  (teardown). In `tests/threads-test.sh` (5° marker).

## BONUS — spiegato il fail pre-esistente di frame-smp-test

Il backtrace del watchdog (ora loggato per intero) mostra la shell window
uccisa dentro `gui_core::raster::Renderer::render` (`roundf`): sotto QEMU TCG
il rendering della shell sfora i 3 s di FRAME_DEADLINE durante il boot del
test → killed → mai 2 finestre awake → marker `frame cores=` mai emesso.
Lentezza da emulazione, non un bug: su HW reale la shell renderizza in ms
(verifica di Giuseppe). Eventuale fix: budget TCG-aware o test marcato
HW/VBox-only — fuori scope qui.

## Verifica

- `THREADS-WIN-OK = ok teardown=ok` su QEMU `-smp 4` E `-smp 1`.
- Regressione CLI dopo il refactor GroupKind: `PARSUM_OK threads=4`,
  `STRESS_MT_OK count=400000`, `PTHREAD_C_OK`, `THREADS_INIT_DONE` (smp 6).
- `make run-test`: TEST_PASS.

## File toccati

- kernel/src/wasm/wt/threads.rs
- kernel/src/wasm/wt/wm.rs
- kernel/src/wasm/wt/mod.rs
- kernel/src/boot/phases/interrupts.rs
- tools/mtwin/ (nuovo)
- tests/threads-test.sh
- Makefile
- docs/superpowers/specs/2026-06-13-wm-threaded-windows-design.md (nuova)
- docs/api/wasi.md
- CLAUDE.md
- CHANGELOG/495-26-06-13-wm-threaded-windows.md
