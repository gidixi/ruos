# 455 — Watchdog epoch sui frame() del compositor (impl spec 454)

**Data:** 2026-06-11

## Cosa

Implementate le fasi 1+2 della spec `2026-06-11-wt-epoch-interruption-design.md`:

- **Engine**: `epoch_interruption(true)` in `engine_config` (kernel) e in
  `tools/wt-precompile` (tunable hashato → byte-match obbligatorio).
- **Sorgente epoch**: ramo BSP di `timer_handler` (100 Hz) → `wt::epoch_tick()`
  (1 epoch = 10 ms). `ENGINE` promossa a static di modulo; `epoch_tick` usa
  `Once::get()` (mai `call_once`) — IRQ-safe, un `fetch_add` Relaxed.
- **Deadline per entry point** (costanti in `wt/mod.rs`): frame a regime
  30 tick (~300 ms), primo frame 300 (~3 s), `_initialize` 300, probe
  `manifest()` 100, store non-compositor (CLI `run_cwasm`, `run_hello`, spike,
  component bring-up) ∞ (`NO_DEADLINE_TICKS` — senza set il default 0 trappa
  subito). NB: valori più larghi della spec (6/50/100) per i tempi dilatati
  10-30× di QEMU TCG; da ristringere dopo taratura su HW reale.
- **`frame_all`**: riarmo deadline prima di ogni `frame.call`;
  `Trap::Interrupt` → log `frame() WATCHDOG (epoch deadline) … killed` +
  `close_requested` (stessa sorte degli altri trap: reap al giro dopo, il
  desktop non si congela mai). `frame_deadline_override` (solo boot-check) per
  i self-test gate/viewer che eseguono un intero benchmark in UN frame.
- **`run_initialize` → bool**: trap (incluso watchdog) in `_initialize` ora
  ABORTISCE lo spawn (prima: log + finestra zombie).
- **Boot-check nuovo**: `tools/wt-spin-reactor` (commit sano al frame 1, loop
  infinito dal frame 2) + `watchdog_self_test` (spinner reaped + reactor sano
  continua a tickare) + riga `epoch watchdog spinner_reaped=… healthy_tick=…`
  nella fase interrupts.
- **Migrazione .cwasm**: ri-AOT di TUTTI gli artefatti — blob embedded
  (hello/echo/cat/spin/gfxtest/bringup via `make wt-cwasm`; reactor/probe/
  egui_demo/shell via le regole di `make iso`; viewer/viewer-gate/spin_reactor
  manuali), `apps/viewer.cwasm`, `ruos-test/deploy/*`. Costo osservato del
  tunable: viewer.cwasm 70.8 → 77.4 MB (~+9 % di codice per i check epoch).
- Docs: `docs/api/README.md` §compatibilità + `apps/README.md` citano
  `epoch_interruption` come secondo incidente-tipo dopo il 422.

## Perché

Un `frame()` pesante o impazzito (relayout Stylo di pagine grandi, loop
infinito) bloccava l'intero desktop per sempre — `frame.call` è sincrono sul
GUI core, senza fuel/async/preemption. Col watchdog il guest colpevole viene
trappato entro ~300 ms e la finestra chiusa; input, present e le altre
finestre continuano. Niente resume cooperativo: richiederebbe la feature
`async` di wasmtime (non nel build) — vedi spec (changelog 454).

## File toccati

- kernel/src/wasm/wt/mod.rs
- kernel/src/wasm/wt/wm.rs
- kernel/src/wasm/wt/component.rs
- kernel/src/timer.rs
- kernel/src/boot/phases/interrupts.rs
- tools/wt-precompile/src/main.rs
- tools/wt-spin-reactor/ (nuovo)
- Makefile
- docs/api/README.md, apps/README.md
- kernel/src/wasm/wt/*.cwasm, apps/viewer.cwasm, ruos-test/deploy/* (rigenerati)
