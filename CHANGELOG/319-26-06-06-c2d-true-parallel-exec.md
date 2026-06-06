# 319 — C2d: esecuzione `.cwasm` veramente parallela (rimosso il lock di serializzazione)

**Data:** 2026-06-06

## Cosa

Due (o più) app `.cwasm` ora eseguono su due (o più) core ComputeApp **allo stesso
tempo di wall-clock** — il vero guadagno di throughput. Fino a C2c ogni `run_cwasm`
era serializzato da `RUN_CWASM_LOCK` perché i primitivi di sincronizzazione di
default di wasmtime no_std vanno in `panic!("concurrent lock request ...")` alla
prima contesa. C2d rimuove quel collo di bottiglia:

1. **Abilitata la feature `custom-sync-primitives` di wasmtime** (`kernel/Cargo.toml`).
   Con essa wasmtime non panica più alla contesa ma chiama 8 funzioni
   `extern "C"` fornite dall'embedder.
2. **Implementati gli 8 shim `wasmtime_sync_*`** in `kernel/src/wasm/wt/platform.rs`
   come spinlock cross-core reali. Lo stato del lock vive INLINE nella cella
   `*mut usize` da 8 byte che wasmtime passa (zero-init = unlocked), trattata come
   `AtomicUsize`. Mutex: 0/1. RwLock: 0 = libero, N = N lettori, `usize::MAX` =
   un writer. Tutti gli spin girano con **INTERRUPTS ABILITATI** (solo
   `core::hint::spin_loop()`, nessun `cli`/mask IF): un core in spin deve poter
   ancora ACK-are le IPI di TLB-shootdown (3d) e servire il timer, altrimenti il
   kernel andrebbe in deadlock sull'attesa di ack dello shootdown.
3. **TLS per-core** in `platform.rs`: `wasmtime_tls_get/set` ora indicizzano un
   array `TLS[MAX_CPUS]` per `cpu_id()` invece di un singolo `AtomicPtr` globale.
   wasmtime tiene lo stato per-attivazione (`CallThreadState`) in TLS; con
   esecuzione concorrente su più core un puntatore globale unico verrebbe
   corrotto tra core.
4. **Rimosso `RUN_CWASM_LOCK`** (`kernel/src/wasm/wt/mod.rs`): la concorrenza è ora
   garantita da custom-sync-primitives + TLS per-core. I lock interni di wasmtime
   sono fine-grained (brevi insert nel registro tipi/moduli, MAI tenuti durante
   l'esecuzione del guest) → vera parallelità con solo brevissima contesa sul
   registro.
5. **Nuovo guest `spin.cwasm`** (`tools/wt-spin/spin.wat`): busy-loop ~2e9 iterazioni
   (~300-800 ms su QEMU) poi esce 0. Regola Makefile aggiunta a `WT_KCWASMS`.
6. **Riscritto il gate `parallel_probe`** (`kernel/src/executor/mod.rs`) perché ora
   chiami davvero `run_cwasm(spin.cwasm)` invece di un loop di pura aritmetica.
   Aggiornato il gate in `interrupts.rs` (`ITERS=1`, commenti corretti). Così
   `overlap=true` diventa la PROVA REALE di esecuzione wasm parallela, non più
   solo parallelismo di calcolo.

## Perché

C2c instradava ogni `.cwasm` su un core ma li serializzava in tempo: nessun
guadagno di throughput reale. Inoltre il vecchio `parallel_probe` misurava un loop
di calcolo puro (non wasmtime), quindi il suo `overlap=true` era fuorviante (il
kernel aveva già il parallelismo di calcolo, mai esercitato wasmtime). C2d rende
la concorrenza wasm reale e il gate onesto.

## Analisi deadlock (registrata per non reintrodurre l'hazard)

Gli shim mmap (`wasmtime_mmap_new`/`_remap`/`_munmap`/`_mprotect`) prendono il
`MAPPER` del kernel solo DENTRO `map_page`/`set_flags`/`unmap_page` — per pagina,
acquisito e rilasciato subito, MAI tenuto attraverso codice di lock del registro
wasmtime. `allocate_frame`/`free_frame` sono leaf lock. Ordine stretto
`wasmtime-reglock ⊃ MAPPER`, mai il contrario → niente ABBA. Regola da mantenere:
NON tenere `MAPPER` attraverso una chiamata wasmtime e NON chiamare wasmtime dentro
una sezione critica di `MAPPER`. Gli spinlock custom NON mascherano mai IF.

## Gate verificato (-smp 4, due run)

```
parallel-exec ran=[2,3] concurrent_ms=2930 single_ms=2880 overlap=true
parallel-exec ran=[2,3] concurrent_ms=2930 single_ms=2870 overlap=true
```

`concurrent_ms ≈ single_ms` (≈ 2930 vs ~2880, NON ≈ 2×=5760) su entrambe le run,
`ran=[2,3]` core distinti, nessun panic/#DF/#PF/#GP/"concurrent lock". Regressione
completa verde: `TEST_PASS`, `TEST_PASS_SMP`, `TEST_PASS_SMP2`, `TEST_PASS_SSH`,
`TEST_PASS_EXEC_AP`.

## File toccati
- kernel/Cargo.toml
- kernel/src/wasm/wt/platform.rs
- kernel/src/wasm/wt/mod.rs
- kernel/src/executor/mod.rs
- kernel/src/boot/phases/interrupts.rs
- tools/wt-spin/spin.wat
- Makefile
- CHANGELOG/319-26-06-06-c2d-true-parallel-exec.md
