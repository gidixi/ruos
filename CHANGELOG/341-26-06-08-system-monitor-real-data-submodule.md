# 341 — System Monitor dati reali: lato ruos-desktop + bump submodule (SP-F)

**Data:** 2026-06-08

## Cosa

Lato submodule `ruos-desktop` (branch `feat/system-monitor-real-data`), la finestra
System Monitor ora mostra telemetria reale del kernel invece dei dati simulati:

- **gui-core** (puro): nuovo `desktop/apps/sysinfo.rs` — trait `SysInfoSource` +
  tipi POD (`CpuSnapshot`/`CoreLoad`/`ProcRow`/`MemSnapshot`) + matematica diff
  testata (`proc_cpu_pct`/`core_util`/`aggregate_util`). `System` riscritto: rende
  dagli snapshot, niente simulazione; tab CPU (per-core + grafico totale) e Memoria
  (frame fisici + heap), tabella processi `Processo | % CPU | Tempo CPU | Memoria |
  PID`. Rimosse le tab Temperatura/Energia. System tolto dal preview PC monolitico.
- **ruos-window**: `RuosSysInfo` implementa `SysInfoSource` sul modulo host `sys`
  (parser blob byte-compatibili col writer kernel).
- **system-app**: costruisce `System::new(Box::new(RuosSysInfo))`.

Bump del puntatore submodule `ruos-desktop` al branch `feat/system-monitor-real-data`.
La parte kernel (host module `sys`) è nelle entry 339; il fix deadlock compositor
collegato è 340.

## Perché

SP-E aveva spedito il System Monitor con dati placeholder; SP-F lo collega ai dati
veri (CPU per-core, processi via `proc::list`, frame fisici, uptime) tramite l'host
module `sys` e la capability `SysInfoSource`, mantenendo `gui-core` puro.

## File toccati
- ruos-desktop (submodule pointer → feat/system-monitor-real-data)
- CHANGELOG/341-26-06-08-system-monitor-real-data-submodule.md
