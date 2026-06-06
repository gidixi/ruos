# 316 — C2a: run_cwasm (WASI app path) runs on a ComputeApp core

**Data:** 2026-06-06

## Cosa

Aggiunto `cwasm_ap_probe` task (in `executor/mod.rs`) + il relativo boot-check
(in `interrupts.rs`) che spawnano il task sul core 2 (ComputeApp) e verificano
che `run_echo_demo()` — la REALE path WASI: engine condiviso + WASI Linker +
argv + Store per-instance + _start — esegua correttamente su un AP.

Statics aggiunti (sotto `#[cfg(feature = "boot-checks")]`):
- `CWASM_AP_RAN_ON: AtomicU32` — quale core ha eseguito il probe
- `CWASM_AP_CODE: AtomicI32` — exit code restituito da `run_echo_demo()`

Gate output (2/2 run, SMP 4, nessun fault):
```
[T+0.327s] INFO wt   wasmtime WASI echo exit=0
[T+0.828s] INFO cwasm-ap ran_on=core2 code=0 spawned=true (expect core2, echo exit code)
```

## Perché

C1 aveva provato che wasmtime AOT gira su un AP (con `run_hello_demo`, path leggera).
C2a prova la path WASI più pesante (`run_cwasm`: WASI Linker, argv, store per-instance,
`_start`) su un core ComputeApp, de-rischiando C2b (routing reale delle app `.cwasm`
exec'd agli AP). Trovato: `run_cwasm` non usa fiber — gira tutto sullo stack chiamante
del task embassy; con `task-arena-size-65536` il rischio stack è basso e il gate lo
conferma senza #DF.

## File toccati
- `kernel/src/executor/mod.rs`
- `kernel/src/boot/phases/interrupts.rs`
- `CHANGELOG/316-26-06-06-c2a-cwasm-wasi-on-compute-ap.md`
