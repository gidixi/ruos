# 476 — MT Fase 1: compositor parallelo + audit rientranza host fn

**Data:** 2026-06-12

## Cosa

Le `frame()` delle finestre WASM sveglie vengono ora eseguite in parallelo sul
compute pool SMP (un core per finestra), invece che serializzate sul solo core
GUI. `Compositor::frame_all` è in 3 fasi:

- **A (core GUI):** `compute_awake` + arming deadline epoch per-store → riempie
  un'arena statica di descrittori `*mut Window` (`FRAME_ARENA`, mirror di
  `BAND_ARENA`).
- **B (parallela):** `dispatch_frames` submitta una `frame()` per finestra al
  compute pool (`frame_one_job`), con fallback inline (≤1 core / pool pieno) e
  join con work-steal — stesso pattern di `dispatch_bands`. Join PRIMA di tornare.
- **C (core GUI, post-join):** adozione committed size + `framed_once`.

Estratta `Compositor::run_frame(&mut Window)` (no `&self`) come corpo della
singola `frame()` (get_typed_func + call + crash safety-net), callable da
qualsiasi core.

Nuova feature `wm-serial-frames`: ripristina l'esecuzione seriale sul core GUI
(baseline bisect, mirror di `serial-composite`). Default = parallelo.

Nuovo marker boot-check `frame cores=N [..]` (gemello di `composite cores=`):
sul primo frame ≥30 con ≥2 job paralleli, riporta i core distinti che hanno
eseguito una `frame()` quel frame (`FRAME_CORE_MASK` resettato per-frame).

**Audit di rientranza** di ogni host fn `wt/*` raggiungibile da `frame()`
(doc dedicato). Unico fix: `proc::REGISTRY` da bare `spin::Mutex` a
`IrqMutex` — `proc::list()` clonava l'intera mappa (alloc) sotto un lock non
interrupt-masked, ora raggiungibile da un AP mentre un altro core fa
`add_cpu_tsc` (violazione regola d'oro #2).

Nuovo test `tests/frame-smp-test.sh`: builda parallelo + seriale, asserisce
`frame cores>=2` (parallelo) e `==1` (seriale).

Fix collaterale `tests/comp-smp-test.sh`: `-m 512` → `-m 2048` (era stale dopo il
bump HEAP_SIZE a 768 MiB → `HeapInit("no usable region")`, il test non bootava).

## Perché

Le app moderne usano il multithreading; ruos lo precludeva. Fase 1 della roadmap
MT (`docs/superpowers/specs/2026-06-12-wasm-multithreading-roadmap-design.md`):
audit-first — rendere il kernel sicuro alle chiamate host concorrenti su un
sistema ancora single-thread-per-app (bug riproducibili), beneficio immediato
(un'app lenta non blocca più il desktop), e prerequisito della Fase 2
(wasm-threads). Bring-up gate verificato: Wasmtime esegue guest + gestisce trap
su un AP (prima volta fuori dal BSP; le band job non chiamano Wasmtime).

Verificato in QEMU `-smp 4 -m 2048`: parallelo `frame cores=2 [1, 2]`, seriale
`frame cores=1 [1]`, `composite cores=4`, nessun panic, desktop renderizza.
Verifica VBox (regola CPU-sensitive, ≥4 vCPU + 2048 MB): OK — desktop fluido,
nessun freeze. Stress reactor (Task 5): saltato (opzionale).

Regressione: `run-test` PASS; band SMP `composite cores=4` in ogni boot (intatto).
Il `comp-smp` screendump-equivalence (`screendump_identical=no`) è un fallimento
**pre-esistente NON causato dal MT**: riprodotto IDENTICO con `wm-serial-frames`
su entrambe le build (frame seriali = comportamento pre-MT), quindi è una
differenza parallel-band vs serial-band (codice `dispatch_bands`, NON toccato) o
nondeterminismo di cattura — era mascherato perché il test non bootava a `-m 512`.
Da triagliare a parte.

NB test GUI: i boot con la GUI richiedono **`-m 2048`** (HEAP_SIZE=768 MiB, vedi
`memory/heap.rs`): con `-m 1024` restano ~72 MiB di frame e le 4 finestre
esauriscono il frame allocator → `commit_fault` no-frame → #PF sul core GUI
(riproducibile IDENTICO anche col build seriale `wm-serial-frames`, quindi NON
una regressione della parallelizzazione — era un errore di sizing del test).
`tests/frame-smp-test.sh` usa `-m 2048`.

## File toccati

- kernel/src/wasm/wt/wm.rs (FRAME_ARENA/FrameArg/FRAME_CORE_MASK/FRAME_JOBS_LAST/
  frame_one_job/run_frame/dispatch_frames; frame_all a 3 fasi; marker frame cores=)
- kernel/Cargo.toml (feature wm-serial-frames)
- kernel/src/proc.rs (REGISTRY: spin::Mutex → IrqMutex)
- tests/frame-smp-test.sh (nuovo)
- docs/superpowers/specs/2026-06-12-hostfn-reentrancy-audit.md (nuovo)
- docs/superpowers/plans/2026-06-12-wasm-mt-fase1-compositor-parallelo.md (piano)
