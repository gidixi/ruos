# SMP Fase 2 — Kernel compute offload pool: Design Spec

**Data:** 2026-06-01
**Branch:** `feature/smp-phase2-executor`

## Contesto

Fase 0 (merged `a40a1f6`) + Fase 1 (merged `36f1fcf`) hanno reso ruos SMP-ready
e avviato gli AP, ora parcheggiati in `hlt` idle. Fase 2 li fa **lavorare in
parallelo reale** al BSP.

### Vincolo scoperto (decide lo scope)

`Fiber::run` (esecuzione `.wasm`) è una **`async fn`**: i suoi host-fn fanno
`.await` su socket/timer/vfs. Far girare un `.wasm` su un AP richiederebbe un
mini-executor + per-CPU timer + STI su ogni AP — molto codice, ampia superficie
di race. Quindi Fase 2 **NON esegue `.wasm` sugli AP**. Gli AP eseguono **job
kernel puro-CPU sincroni** (nessun `.await`, nessun executor, nessun STI/IPI):
dimostra il pool di worker SMP end-to-end con rischio minimo.

### Allineamento col pivot

Il pivot impone "async cooperative, single-CPU" per lo userland. Questo design
**lo rispetta**: il BSP resta l'unico executor cooperativo (invariato — shell,
SSH, pipe, exec, net girano esattamente come ora). Gli AP NON sono un secondo
executor né uno scheduler preemptive; sono un pool di worker per lavoro
kernel CPU-bound. Niente preemption, niente run-queue di task async sugli AP.

## Obiettivo

- Coda di job SMP-safe; gli AP (oggi idle) diventano worker che eseguono job
  kernel puro-CPU dalla coda, in parallelo.
- Il BSP sottomette job e raccoglie i risultati; speedup misurabile ≈ min(K, n_AP).
- Nessuno `.wasm` sugli AP, nessun STI/IPI/per-CPU timer sugli AP (spin-wait con
  `pause`), nessuna modifica all'executor cooperativo del BSP.
- Boot + tutti i test esistenti verdi; verificato su QEMU -smp 4 + VBox reale.

## Non-goal (esplicitamente FUORI)

- `.wasm` sugli AP (servirebbe mini-executor — fuori scope).
- STI / IPI-wake / per-CPU LAPIC timer sugli AP (gli AP fanno spin-wait con
  `pause`; vero idle con IPI-wake = follow-up).
- Scheduler preemptive, context switch, work-stealing, migrazione task.
- Executor SMP del BSP (resta single-core cooperativo, INTOCCATO).
- Job che fanno I/O o toccano stato mutabile condiviso (i job sono puro-calcolo
  su input immutabile — vincolo documentato + rispettato dal test).

## Honest ceiling

Dimostra parallelismo SMP REALE per lavoro kernel CPU-bound (es. hash/checksum
di buffer su N core insieme). NON parallelizza il workload reale di ruos
(.wasm/SSH/GUI), che è I/O-bound — quello resta sul BSP. È la prova che il
multi-core fa lavoro utile + l'infrastruttura (coda, worker, raccolta risultati)
su cui un futuro offload più ricco potrebbe costruire. Gli AP in spin-wait
bruciano CPU quando non c'è lavoro (accettabile per un pool; idle vero =
follow-up con IPI).

## Componenti

### 1. `kernel/src/smp/pool.rs` (nuovo) — coda job + slot

Job = funzione puro-CPU + input immutabile + slot risultato. Per evitare
fn-pointer+lifetime complessi, usare uno slot statico con id:

```rust
const MAX_JOBS: usize = 64;

/// A unit of pure-CPU work. `work` must NOT do I/O, block, or touch shared
/// mutable kernel state — it runs synchronously on a worker core.
struct JobSlot {
    state: AtomicU8,            // EMPTY / QUEUED / RUNNING / DONE
    work: AtomicPtr<()>,       // fn(*const u8, usize) -> u64 as raw ptr
    input_ptr: AtomicUsize,
    input_len: AtomicUsize,
    result: AtomicU64,
    ran_on: AtomicU32,         // cpu_id that executed it (proves parallelism)
}

static SLOTS: [JobSlot; MAX_JOBS] = /* zeroed */;
static QUEUE: IrqMutex<VecDeque<usize>> = IrqMutex::new(VecDeque::new()); // slot ids
```

API:
- `pub fn submit(work: fn(&[u8]) -> u64, input: &'static [u8]) -> Option<usize>`
  — claim a free slot, store work+input, push id to QUEUE, return slot id (or
  None if full). Input must be `'static` (e.g. a leaked/kernel buffer) so it
  outlives the async job.
- `pub fn take() -> Option<usize>` — pop a slot id from QUEUE (called by AP
  workers). CAS state QUEUED→RUNNING.
- `pub fn complete(slot, result, cpu)` — store result+ran_on, state→DONE.
- `pub fn poll_done(slot) -> Option<(u64, u32)>` — BSP reads result+ran_on if
  DONE, frees the slot (state→EMPTY).

Use `IrqMutex` (Fase 0) for the QUEUE; atomics (SeqCst) for slot fields. The
work fn pointer stored as `AtomicPtr` (transmute fn→ptr→fn at call — document
the safety: the fn is a `fn(&[u8])->u64` with no captures).

### 2. `kernel/src/cpu/ap.rs` — worker loop instead of pure idle

Today `ap_entry` ends in `loop { hlt }`. Fase 2: after the per-core setup
(gdt/idt/online — unchanged), enter the worker loop:

```rust
pub unsafe extern "C" fn ap_entry(info: &MpInfo) -> ! {
    let cpu_id = info.extra_argument() as usize;
    crate::gdt::init(cpu_id);
    crate::idt::load();
    crate::cpu::mark_online();
    ap_worker_loop()   // never returns
}

fn ap_worker_loop() -> ! {
    let me = crate::cpu::cpu_id();
    loop {
        match crate::smp::pool::take() {
            Some(slot) => crate::smp::pool::run_slot(slot, me),
            None => core::hint::spin_loop(), // pause; no STI/IPI in Fase 2
        }
    }
}
```
`run_slot` reads the work fn + input from the slot, calls it, then
`pool::complete(slot, result, me)`. NO STI (APs take no interrupts). The
spin-loop with `spin_loop()` (PAUSE) is the wake mechanism — the AP polls the
queue. (IPI-wake idle is a follow-up.)

### 3. BSP-side dispatch + a demo/test path

A way to submit jobs and measure parallelism. Simplest: a new host fn
`ruos_smp_bench(n_jobs, iters) -> errno` + a `smptest` user tool, OR a kernel
self-test behind the `boot-checks` feature. Recommended: a small `smptest.wasm`
tool calling a host fn that:
- builds N identical pure-CPU jobs (e.g. a fixed-iteration integer hash over a
  buffer),
- times running them via the pool (parallel) vs running them inline on the BSP
  (sequential),
- returns both timings + the set of `ran_on` cpu_ids (proving they ran on
  different cores).
The tool prints: `parallel=Xms sequential=Yms speedup=Z.Zx cores=[0,1,2,3]`.

The job work fn: a deterministic CPU-bound loop, e.g.
```rust
fn hash_job(input: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325; // FNV-ish, then heavy mixing loop
    for _ in 0..ITERS {
        for &b in input { h = (h ^ b as u64).wrapping_mul(0x100000001b3); }
        h = h.rotate_left(13).wrapping_add(0x9e3779b97f4a7c15);
    }
    h
}
```
ITERS tuned so one job takes ~tens of ms (long enough to measure, short enough
for tests).

## Gestione errori

- QUEUE/slots full → `submit` returns None; the BSP runs that job inline
  (degrade gracefully, still correct).
- No AP online (1 CPU) → `submit` still works but nobody `take`s; the BSP must
  drain the queue itself inline (or the bench detects `cpus_online()==0` and
  runs sequential only). Document: with 0 APs the pool falls back to BSP-inline.
- A job fn that panics → the survivable panic handler (Fase A) catches it; but
  pure-CPU jobs shouldn't panic. Jobs must be `catch`-free pure functions.
- Slot state races → CAS (compare_exchange) on `state` ensures exactly one
  worker runs a QUEUED slot.

## Strategia di test

- `smptest` tool: N hash jobs, parallel vs sequential timing + ran_on cpu set.
- QEMU `-smp 4`: assert `speedup >= 1.5` (3 APs available) AND `ran_on` contains
  ≥2 distinct cpu_ids → proves real parallel execution. Marker e.g.
  `smptest: speedup=2.7x cores=4`.
- QEMU `-smp 1`: pool falls back to BSP-inline; speedup ≈ 1.0, no crash.
- VBox reale (6 CPU): higher speedup; assert ≥2 distinct cores + no #PF.
- run-test / ssh / pipe / fuel / smp all green (BSP executor untouched).
- New `make run-smp2-test` (QEMU -smp 4, assert speedup + distinct cores).

## Done criteria

- AP workers execute pure-CPU jobs from the SMP queue in parallel with the BSP.
- `smptest` shows speedup ≥1.5× on -smp 4 and ≥2 distinct `ran_on` cpu_ids.
- 1-CPU fallback: pool drains inline on the BSP, no crash, speedup ≈1.
- No `.wasm` on APs, no STI/IPI on APs, BSP executor unchanged.
- All existing tests green + run-smp2-test green.
- Verified on real VirtualBox (no #PF, parallel speedup).

## Piano implementativo (sintesi — dettaglio in writing-plans)

1. `smp/pool.rs`: JobSlot array + IrqMutex queue + submit/take/run_slot/complete/
   poll_done.
2. `cpu/ap.rs`: ap_worker_loop replaces the idle hlt loop.
3. Host fn `ruos_smp_bench` + the hash job + parallel/sequential timing + ran_on.
4. `smptest` user tool (prints speedup + cores) + build wiring.
5. `make run-smp2-test` (QEMU -smp 4) + VBox verify + roadmap nota.
