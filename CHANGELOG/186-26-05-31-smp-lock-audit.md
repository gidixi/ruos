# 186 — SMP lock audit: classify every sync site, document executor invariant

**Data:** 2026-05-31

## Cosa
Full audit of EVERY shared-state synchronization site in the kernel for SMP
safety. Read the surrounding code of each site (not just the grep line) and
classified into a markdown table at
`docs/superpowers/notes/2026-05-31-smp-lock-audit.md`.

Result (total ~52 sites):
- **SAFE-AS-IS** (spinlock-backed): 27 `Mutex` statics + every
  `without_interrupts(|| mutex.lock())` wrapper — `spin::Mutex` already gives
  cross-core exclusion; the IF-masking only prevents same-core ISR deadlock.
  Verified each `without_interrupts` site actually wraps a `spin::Mutex.lock()`
  (net/*, pty/*, pipe/*, vfs/devices, banner, log, kprint, console_drain).
- **SAFE-BSP-ONLY / INIT-ONCE**: 7 — the `static mut` sites (`IOAPIC_VIRT`,
  `LAPIC_VIRT` written once at init then read-only; `PER_CPU`, `DOUBLE_FAULT_STACK`,
  `TSS` per-core-partitioned arrays each touched only on `[cpu_id]` during that
  core's boot) plus init-once atomics (`BOOT_TSC`/`TSC_PER_MS`). The
  `&'static mut PageTable` in mapper.rs is a function-local sole-owner serialized
  by `MAPPER` (spin::Mutex).
- **SAFE-ATOMIC**: 18 atomic statics with correct ordering (`WAKE_PENDING` SeqCst,
  fb geometry Release/Acquire, keyboard modifier atomics ISR-local, `TICKS`/
  `NEXT_PID`/`GEN_COUNTER` monotonic Relaxed, pty `CLAIMED` CAS SeqCst).
- **MUST-FIX**: **0**.

**No conversions performed** — zero genuinely-unsafe sites. All shared mutable
state is already spinlock-backed, atomic with correct ordering, or
init-once / per-core by construction. `IrqMutex` (Task 1) remains available for
future ISR-shared state; `delay.rs`/`pty.rs`/`pipe.rs` are natural future
candidates to migrate to it for ergonomics but are SMP-correct today (YAGNI).

Also (comment-only, no functional change): expanded the
`unsafe impl Sync for ExecCell` SAFETY comment in `executor/mod.rs` to state the
single-core invariant explicitly — exactly one core (BSP) calls `run()`/`poll()`;
the cooperative executor is single-core by the 2026 pivot; the run-queue is NOT
yet SMP-safe; a future SMP phase that starts an AP MUST revisit this assertion.

## Perché
Task 5 of SMP phase-0: make the kernel STRUCTURALLY ready for SMP by auditing
every sync site so an unprotected shared mutation can't silently corrupt under a
second core. The honest, complete audit (with documented invariants) is the
deliverable; the codebase already used `spin::Mutex`/atomics correctly, so no
dangerous site needed converting.

## File toccati
- docs/superpowers/notes/2026-05-31-smp-lock-audit.md (new)
- kernel/src/executor/mod.rs (SAFETY comment only)
- CHANGELOG/186-26-05-31-smp-lock-audit.md (new)
