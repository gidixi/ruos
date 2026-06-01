# 196 — ruos_smp_bench host fn

**Data:** 2026-06-01

## Cosa
Nuovo host fn `ruos/smp_bench(buf_ptr, buf_len, used_ptr) -> errno`.

Esegue N job hash puri (FNV-1a + mixing, 4_000_000 iterazioni ciascuno) sia via
il pool SMP (parallel across APs) che inline sul BSP (sequential), cronometra
entrambi con `boot::clock::elapsed_ms()` (TSC-based, lock-free), e scrive nel
buffer guest una riga ASCII:

```
parallel=Xms sequential=Yms speedup=Z.ZZx cores=[a,b,c]
```

Fallback 1-CPU: se `cpus_online() == 0` il bench drena i job dal pool inline sul
BSP (via `pool::take` + `pool::run_slot`) prima di raccogliere i risultati, così
non si blocca in `poll_done` quando nessun AP è attivo.

`write_bytes_and_len` in `sysinfo.rs` promosso a `pub(crate)` per poter essere
riusato da `smp.rs`.

## Perché
Task 3 di SMP Fase 2: host fn misurabile che dimostra il guadagno di parallelismo
del pool AP vs esecuzione sequenziale sul BSP. Necessario per Task 4 (wasm tool
`smplbench`).

## File toccati
- kernel/src/wasm/host/smp.rs (nuovo)
- kernel/src/wasm/host/mod.rs (pub mod smp + smp::link)
- kernel/src/wasm/host/sysinfo.rs (write_bytes_and_len → pub(crate))
