# 315 — C1: WASM (wasmtime AOT) instance runs correctly on a ComputeApp core

**Data:** 2026-06-06

## Cosa
Aggiunto il probe di boot-check C1 che dimostra che il runtime wasmtime AOT
istanzia ed esegue correttamente una app WASM su un core non-BSP (ComputeApp,
core 2).

Modifiche:
- `kernel/src/executor/mod.rs`: aggiunti `WASM_AP_RAN_ON` e `WASM_AP_OK`
  (AtomicU32, `#[cfg(feature = "boot-checks")]`) e il task
  `wasm_ap_probe` (pub, `#[embassy_executor::task]`) che chiama
  `crate::wasm::wt::run_hello_demo()` e poi scrive il proprio `cpu_id()` e
  il risultato negli statici.
- `kernel/src/boot/phases/interrupts.rs`: aggiunto il blocco C1 in fondo alla
  sezione `boot-checks`; fa `spawn_on(2, wasm_ap_probe())` (con retry fino a
  che il SendSpawner è pubblicato), poi aspetta in spin fino a `WASM_AP_OK != 2`,
  e stampa `wasm-ap ran_on=core{} ok={} spawned={}`.

Nessuna modifica al task-arena-size (65536 è sufficiente per l'hello probe).

## Perché
De-risk per C2: prima di instradare app reali exec'd sui core ComputeApp
bisognava verificare che il runtime wasmtime AOT funzionasse off-BSP senza
crash (#DF da stack overflow) né errori di stato globale. Il gate mostra
`ran_on=core2 ok=1 spawned=true` su entrambe le esecuzioni (SMP 4) senza
fault — C2 è sbloccato.

## File toccati
- kernel/src/executor/mod.rs
- kernel/src/boot/phases/interrupts.rs
- CHANGELOG/315-26-06-06-c1-wasm-ap-probe.md
