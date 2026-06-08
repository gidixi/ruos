# 338 — Spec SP-F: System Monitor con dati reali

**Data:** 2026-06-08

## Cosa

Scritta la spec di design `docs/superpowers/specs/2026-06-08-system-monitor-real-data-design.md`:
rendere reale la finestra egui System Monitor (oggi 100% simulata con sinusoidi).

- Architettura: capability trait `SysInfoSource` in `gui-core` (puro), impl reale
  `RuosSysInfo` in `ruos-window`, nuovo modulo host kernel `kernel/src/wasm/wt/sys.rs`
  (modulo `"sys"`) sul linker Wasmtime del compositor — riusa i blob di rtop.
- Dati reali: CPU per-core (busy/idle), processi (`proc::list`: pid/nome/cpu_tsc/
  mem_bytes), memoria (heap + frame), uptime. CPU% per-processo per differenza di
  snapshot nel tempo.
- Tabella processi: `Processo | CPU% | Tempo CPU | Memoria | PID`.
- Rimosse le tab Temperatura ed Energia (nessun sensore termico nel kernel).
- Anteprima PC: System rimosso dal preview monolitico (niente sorgente simulata).

## Perché

SP-E aveva spedito i quattro desktop app come finestre con dati placeholder e
rinviato i dati reali a SP-F. Questa è SP-F, limitata al System Monitor.

## File toccati
- docs/superpowers/specs/2026-06-08-system-monitor-real-data-design.md
- CHANGELOG/338-26-06-08-system-monitor-real-data-spec.md
