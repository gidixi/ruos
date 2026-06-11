# 454 — spec: epoch interruption Wasmtime (watchdog sui frame() del compositor)

**Data:** 2026-06-11

## Cosa

Nuova spec di design
`docs/superpowers/specs/2026-06-11-wt-epoch-interruption-design.md`: introdurre
`epoch_interruption` di Wasmtime come **watchdog** sui `frame()` sincroni del
compositor. Epoch incrementato dal timer IRQ del BSP (100 Hz → 1 tick = 10 ms),
deadline per entry point (frame a regime 6 tick, primo frame 50, `_initialize`
100, probe manifest 10, tool CLI ∞), trap `Trap::Interrupt` distinto in
`frame_all` → log `WATCHDOG` + chiusura della finestra colpevole. Resume
cooperativo respinto (richiede feature `async`/fiber, assente nel build
runtime-only no_std). Include censimento di tutti i siti `Store::new` (deadline
0 = trap immediato), regola byte-match kernel ↔ `tools/wt-precompile`, piano di
migrazione re-AOT di TUTTI i `.cwasm` (embedded, `/bin`, `apps/`, `/mnt/apps`),
rischi (falsi positivi su primi frame Blitz, overhead check ~1-3 % da misurare
col GATE), piano di test (app spinner + boot-checks) e fasi 1-3.

## Perché

Item D dell'analisi prestazioni: un `frame()` guest pesante o in loop (Blitz
6400 nodi = 28 ms misurati, 50k ≈ 220 ms estrapolati) blocca l'intero desktop —
il compositor chiama i frame in modo sincrono senza alcun meccanismo di
interruzione (`engine_config` non ha epoch/fuel/async). Il watchdog trasforma
"desktop congelato per sempre" in "finestra colpevole chiusa entro ~50 ms".

## File toccati

- docs/superpowers/specs/2026-06-11-wt-epoch-interruption-design.md (nuova)
- CHANGELOG/454-26-06-11-spec-wt-epoch-interruption.md (questa entry)
