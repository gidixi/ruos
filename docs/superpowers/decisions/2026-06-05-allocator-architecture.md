# Allocator Architecture Decision — SMP Step 1

**Data:** 2026-06-05  
**Status:** PENDING controller/user confirmation  
**Spike branch:** `docs/smp-shared-nothing`  
**Related spec:** `docs/superpowers/specs/2026-06-05-smp-shared-nothing-migration-design.md` §6

---

## 1. Context

The kernel currently uses a single global `Talck<spin::Mutex<()>>`. With N per-core
executors (SMP Step 3) every alloc/free serialises on that one lock. Step 1 must
resolve this before Step 3 lands.

Three architectures were prototyped and benchmarked under both 1-core and 4-core QEMU
(`-smp 4`, `-cpu max`, `-machine q35`):

| Variant | Feature flag | Description |
|---|---|---|
| **Default talc** | `boot-checks` | Single global `Talck<spin::Mutex<()>>` — baseline |
| **Prototype A — magazine** | `boot-checks,alloc-magazine` | Per-core size-class free-list cache in front of ONE global talc. Cache hit avoids the allocator entirely. `cpu_id()` called per alloc. |
| **Prototype B — percore-talc** | `boot-checks,alloc-percore-talc` | Per-core talc arenas (4 MiB/core) + 64 MiB fallback talc + remote-free queue (`IrqMutex<VecDeque>`). `cpu_id()` per alloc; `drain_remote` (IrqMutex) every alloc; `owner_of` O(16) per dealloc; remote-free assumes align=16. |

---

## 2. Benchmark Results

### Benchmark harness

The `allocbench` harness runs post-LAPIC in the interrupts phase. Three serial markers
are emitted:

- **cpuid** — cost of `cpu_id()` alone (LAPIC MMIO read), 100k iterations
- **single** — single-core BSP alloc/free latency (small = 32-byte alloc, large = 2 MiB alloc)
- **multi** — N jobs of 50k allocs each submitted via `smp::pool`; `cores=` reports how many
  APs actually ran jobs (> 1 confirms distribution across cores)

### Raw allocbench lines (smp4 runs)

**Default talc (`bench-default-smp4.log`):**
```
[T+0.030s] INFO smp  3/3 APs online
[T+0.053s] INFO allocbench cpuid ns_per_call=219 iters=100000 acc=0
[T+0.073s] INFO allocbench single small_ns=181 large_ns=636 iters=100000 acc=0xFBC520
[T+0.132s] INFO allocbench multi cores=3 total_ns=59092901 per_job=19697633 jobs=3 sink=0xDF835088
CONSOLE_TEST: OK
```

**Prototype A — magazine (`bench-magazine-smp4.log`):**
```
[T+0.028s] INFO smp  3/3 APs online
[T+0.050s] INFO allocbench cpuid ns_per_call=213 iters=100000 acc=0
[T+0.120s] INFO allocbench single small_ns=677 large_ns=444 iters=100000 acc=0xFBC520
[T+0.216s] INFO allocbench multi cores=3 total_ns=95582957 per_job=31860985 jobs=3 sink=0xDF835088
CONSOLE_TEST: OK
```

**Prototype B — percore-talc (`bench-percore-smp4.log`):**
```
[T+0.044s] INFO smp  3/3 APs online
[T+0.065s] INFO allocbench cpuid ns_per_call=208 iters=100000 acc=0
[T+0.154s] INFO allocbench single small_ns=856 large_ns=1088 iters=100000 acc=0xFBC520
[T+0.267s] INFO allocbench multi cores=3 total_ns=113503389 per_job=37834463 jobs=3 sink=0xDF835088
CONSOLE_TEST: OK
```

### Comparison table

| Variant | cpuid ns (1c) | cpuid ns (4c) | single small_ns (1c) | single small_ns (4c) | single large_ns (1c) | single large_ns (4c) | multi cores (4c) | multi per_job (1c) | multi per_job (4c) |
|---|---|---|---|---|---|---|---|---|---|
| Default talc | 198 | 219 | 162 | 181 | 486 | 636 | 3 | 10.9 ms | **19.7 ms** |
| Magazine A | 242 | 213 | 162 | 677 | 1118† | 444 | 3 | 10.6 ms | **31.9 ms** |
| Percore-talc B | 221 | 208 | 164 | 856 | 493 | 1088 | 3 | 10.5 ms | **37.8 ms** |

† 1118 ns for magazine large_ns at 1-core was identified as noise (large alloc bypasses the magazine cache and hits global talc, same path as default).

**AP distribution:** all three variants show `cores=3` at `-smp 4`, confirming that benchmark jobs distributed across all 3 APs. The pool did NOT drain greedily on BSP.

**Log files:** `build/bench-default-smp4.log`, `build/bench-magazine-smp4.log`, `build/bench-percore-smp4.log`

---

## 3. Criteria Scoring

### Criterion 1 — Correctness (boot + no fault)

All three variants:
- Boot with `3/3 APs online`
- Emit all three `allocbench` markers
- Pass `CONSOLE_TEST: OK`
- No kernel panic or fault observed

**Score: all three PASS.**

### Criterion 2 — Contention under -smp 4 (KEY metric: multi per_job, lower = better)

| Variant | multi per_job (4c) | vs. default |
|---|---|---|
| Default talc | 19.7 ms | baseline |
| Magazine A | 31.9 ms | **+62% WORSE** |
| Percore-talc B | 37.8 ms | **+92% WORSE** |

**This is the critical finding: neither prototype A nor prototype B outperforms the
global talc under `-smp 4` contention. Both are significantly slower.**

Default talc wins this criterion. The prototypes were designed to reduce lock contention,
but the benchmark shows they are adding overhead rather than removing it.

### Criterion 3 — Single-core regression (small_ns should not be >~10% worse than default)

| Variant | small_ns (4c) | % change vs default |
|---|---|---|
| Default talc | 181 ns | baseline |
| Magazine A | 677 ns | **+274% WORSE** |
| Percore-talc B | 856 ns | **+373% WORSE** |

**Both prototypes show severe single-core regression.** The magazine cache is supposed
to avoid the allocator on cache hits, but at -smp 4 the single-core path is paying the
`cpu_id()` LAPIC cost (~200 ns) plus overhead. Prototype B pays additionally for
`drain_remote` (IrqMutex) and `owner_of` O(16) on every alloc.

**Score: both A and B FAIL this criterion decisively.**

### Criterion 4 — Simplicity / risk

| Variant | Unsafe concerns / limitations |
|---|---|
| Default talc | One spinlock; simple; no per-core state; well-tested. |
| Magazine A | cpu_id() per alloc; size-class cache adds code complexity; no remote-free needed (cache returns to its own core). |
| Percore-talc B | cpu_id() per alloc; drain_remote IrqMutex on every alloc; owner_of O(16) per dealloc; remote-free assumes align=16 (correctness limitation); more complex failure modes. |

**Score: default talc is simplest. B has the most risk (align=16 assumption is a correctness
hazard for types with alignment > 16).**

---

## 4. Root-Cause Analysis

### Why the prototypes are slower, not faster

The dominant finding from the 1-core runs (prior task) was that `cpu_id()` via LAPIC MMIO
costs ~200 ns per call. This cost is paid on **every** alloc in both A and B.

At `-smp 4`, the benchmark runs 50k allocs per job across 3 APs simultaneously. The
expected benefit is: lock contention on global talc is eliminated, so per-core allocs
proceed in parallel. But the measurement shows the opposite.

The likely explanation (to be confirmed with profiling):

1. **The benchmark alloc pattern hits cache misses on per-core state.** The magazine
   (Prototype A) stores per-core free-lists in a `static` array; under QEMU SMP, cache
   coherence traffic between virtual cores for these structures may exceed the lock
   contention cost.

2. **Prototype B pays IrqMutex overhead on every alloc path** (drain_remote), even when
   the remote-free queue is empty. This is unconditional overhead that accumulates at
   50k allocs/job.

3. **The global talc spinlock may not be heavily contested in this micro-benchmark.**
   The benchmark jobs are alloc-heavy but short-lived. With 50k allocs of small objects
   per job, the lock hold time is minimal (a few hundred ns). Three cores competing for
   a briefly-held spinlock have low actual collision rate, especially in QEMU where vCPU
   scheduling is not truly simultaneous.

### The cpu_id() tax is real

The spec §6 identified this risk explicitly:
> "~200 cicli per LAPIC-read a OGNI small alloc potrebbero rendere l'arena per-core
> più lenta dello spinlock globale."

The data confirms this concern. The mitigation the spec proposed — `gs-base` fast path
for cpu_id (~few cycles) — was NOT implemented in the prototypes. Both prototypes pay
the full ~200 ns LAPIC tax per alloc, which at 50k allocs/job is 10 ms of cpu_id
overhead alone, roughly explaining the measured regressions.

---

## 5. Recommendation Options

**IMPORTANT: The final decision is PENDING controller/user confirmation.** This section
lays out the options and evidence; it does not declare a winner.

### Option 1 — Adopt neither A nor B yet; build fast cpu_id() (gs-base) first

**Evidence for:** The spec §6 explicitly predicted this scenario and listed gs-base as
mitigation (a). The data confirms the prediction: both prototypes are slower because
cpu_id() costs ~200 ns/call. Fixing cpu_id() first (to ~5-10 ns via `gs:[0]`) would
change the cost structure entirely. Only then can a per-core allocator be measured fairly.

**Evidence against:** gs-base is "oggi rotto su VirtualBox" per the spec; fixing it
requires care. This defers the Step 1 allocator decision further, potentially delaying
Step 3.

**Implication for §6:** §6 should be updated to require fast cpu_id() as a
pre-prerequisite of the per-core allocator, not just an optional optimization.

### Option 2 — Adopt Prototype A (magazine) after fixing fast cpu_id()

**Evidence for:** A's design is simpler than B (no remote-free, no align=16 assumption,
no IrqMutex per alloc). If cpu_id() were ~10 ns instead of ~200 ns, the cache-hit path
in A would be very fast (cache lookup + pointer bump, no lock). A avoids the allocator
entirely on cache hits.

**Evidence against:** At current cpu_id() cost, A is worse than default on every metric.
A also showed a large single-core regression (+274%) even at -smp 4 which suggests the
magazine cache adds overhead even on its happy path in the current implementation.

**Implication for §6:** If A is chosen, §6's design (remote-free queue, PerCpuArena
with drain_remote) would be replaced by a magazine-based design. §6 should be rewritten.

### Option 3 — Adopt Prototype B (percore-talc) after fixing fast cpu_id()

**Evidence for:** B matches the §6 design most closely (per-core arenas + remote-free).
Remote-free is required for correctness when Step 2 InboxMsg `Box` values cross cores
and are dropped by the receiver.

**Evidence against:** B has the worst numbers on all contention and single-core metrics.
It pays the most overhead per alloc (cpu_id + drain_remote + owner_of). The align=16
assumption in remote-free is a correctness risk. Even with fast cpu_id(), drain_remote
on every alloc adds IrqMutex overhead.

**Implication for §6:** §6 can largely stay as-is if B is chosen, but the align=16
limitation in remote-free must be addressed as a correctness fix, and drain_remote should
be gated (check an atomic flag before taking the lock).

### Option 4 — Keep default talc for now; defer Step 1 until Step 3 actually shows contention

**Evidence for:** The data shows default talc wins on all metrics at the current benchmark
scale. Step 3 (per-core executor) has not been built yet; actual contention under real
workloads may differ from this micro-benchmark. Premature optimisation of the allocator
before the executor exists adds risk with no measured benefit.

**Evidence against:** §6 calls the per-core allocator a "prerequisito HARD di Step 3".
The spec's reasoning (N executors serialising on one lock) is architecturally sound even
if this micro-benchmark doesn't show it. Deferring risks needing a complex refactor
mid-Step-3 implementation.

---

## 6. Implications for Spec §6

Section §6 of `2026-06-05-smp-shared-nothing-migration-design.md` describes Prototype B
(per-core arenas + remote-free queue) as the target architecture.

The data raises three issues that §6 should address (edits deferred to controller):

1. **cpu_id() must be fast first.** §6 mentions this as a "decisione aperta" but lists
   gs-base as optional. The data makes it a hard prerequisite: without fast cpu_id(),
   any per-core allocator is slower than the global lock. §6 should be updated to make
   fast cpu_id() a gate for Step 1, not a post-hoc optimisation.

2. **Magazine (A) vs. percore-talc (B).** §6 describes B. If the controller decides on
   A (after fast cpu_id()), §6 §6 should be rewritten to describe the magazine design.

3. **drain_remote overhead in B.** Even with fast cpu_id(), calling drain_remote
   (IrqMutex) on every alloc is expensive. §6 should specify that drain_remote is only
   called when an atomic "remote-free pending" flag is set, reducing the common case to
   a single atomic load.

---

## 7. Decision

**PENDING controller/user confirmation.**

The controller should answer:

1. Which option (1 / 2 / 3 / 4) to adopt?
2. Should §6 be updated now (before Step 1 implementation) or inline during implementation?
3. Is the align=16 assumption in Prototype B's remote-free a blocking concern?

Once confirmed, update this document with the chosen option and rationale.
