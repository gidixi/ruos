# SMP lock audit — every shared-state synchronization site

**Date:** 2026-05-31
**Branch:** `feature/smp-phase0-percpu`
**Task:** SMP-foundations Task 5. Audit EVERY shared-state sync site, classify
SMP-safe vs must-fix, convert ONLY the dangerous ones.

## Method

Enumerated all candidate sites via grep for `without_interrupts`, `static mut`,
`Mutex` statics, and `Atomic*` statics under `kernel/src/`. Then **read the
surrounding code of each site** to classify by what actually protects the state,
not by the grep line alone.

## Verdict legend

- **SAFE-AS-IS** — `spin::Mutex` (directly or wrapped in `without_interrupts`).
  The spinlock already gives cross-core mutual exclusion; the `without_interrupts`
  wrapper only prevents same-core ISR-vs-task deadlock. Correct under SMP. Keep.
- **SAFE-BSP-ONLY / INIT-ONCE** — written only during single-threaded boot, then
  read-only (or per-core partitioned). Document the invariant. Keep.
- **SAFE-ATOMIC** — `static Atomic*` with appropriate ordering. Fine under SMP.
- **MUST-FIX** — `static mut` mutated at runtime with no lock, OR shared mutable
  state guarded ONLY by `without_interrupts` with NO spinlock underneath
  (IF-masking does NOT protect cross-core). Convert.

## Result summary

| Verdict | Count |
|---|---|
| SAFE-AS-IS (spinlock-backed) | 27 Mutex statics + all `without_interrupts` wrappers |
| SAFE-BSP-ONLY / INIT-ONCE | 7 (`static mut` ×5 + init-once atomics ×2 groups) |
| SAFE-ATOMIC | 18 atomic statics |
| **MUST-FIX** | **0** |

**No unprotected shared mutable state found.** Every site is spinlock-backed,
atomic with correct ordering, or init-once / per-core by construction. `IrqMutex`
(Task 1) is available for any future ISR-shared state, and `delay.rs` /
`pty.rs` / `pipe.rs` are the natural future candidates to migrate from the
ad-hoc `without_interrupts(|| mutex.lock())` pattern to `IrqMutex` for ergonomics
— but they are already SMP-correct today, so per YAGNI they are NOT rewritten.

## `static mut` sites (the high-risk category) — all SAFE

| site | invariant | verdict |
|---|---|---|
| `apic/ioapic.rs:10` `IOAPIC_VIRT` | written once in `init()` during boot, read-only after | SAFE-BSP-ONLY |
| `apic/lapic.rs:25` `LAPIC_VIRT` | written once in `init()` during boot, read-only after | SAFE-BSP-ONLY |
| `cpu/mod.rs:34` `PER_CPU` | per-core array; each core writes only its own slot at `init_bsp`/future `init_ap`, then reads its own slot via gs-base. `PerCpuArray` asserts `Sync` with documented partition invariant | SAFE-BSP-ONLY (per-core) |
| `gdt.rs:21` `DOUBLE_FAULT_STACK` | per-core `[[u8]; MAX_CPUS]`; each core touches only `[cpu_id]` during its boot | SAFE-BSP-ONLY (per-core) |
| `gdt.rs:27` `TSS` | per-core `[TaskStateSegment; MAX_CPUS]`; each core writes only `[cpu_id]` during its boot, GDT built via `spin::Once` per core | SAFE-BSP-ONLY (per-core) |
| `memory/mapper.rs:65` `&'static mut PageTable` | NOT a static — a `let` binding in `init()`. Sole `&mut` walker of PML4; all PML4 mutation flows through `MAPPER` (`spin::Mutex`). Documented at the site | SAFE (spinlock-serialized) |

## Mutex statics — all SAFE-AS-IS (`spin::Mutex`)

| site | protects | ISR-touched? | verdict |
|---|---|---|---|
| `ahci/mod.rs:22` `PORT0` | AHCI port | no | SAFE-AS-IS |
| `boot/phases/mod.rs:17` `ACPI` | ACPI info handoff (boot only) | no | SAFE-AS-IS (also init-once in practice) |
| `console/mod.rs:61` `CONSOLE` | multi-console writer | yes (kprintln from ISR) — wrapped in `without_interrupts` at every call site | SAFE-AS-IS |
| `executor/delay.rs:40` `SLOTS_LIST` | delay wake slots | **yes** — task side `without_interrupts(\|\| lock())`, ISR side `try_lock()` | SAFE-AS-IS (canonical ISR-shared spinlock) |
| `executor/mod.rs:287` `PTY_QUEUE` | ssh shell spawn queue | no | SAFE-AS-IS |
| `klog.rs:56` `LOG` | dmesg ring; `try_push` for panic | indirectly via log paths | SAFE-AS-IS |
| `memory/frames.rs:144` `FRAMES` | frame allocator | no | SAFE-AS-IS |
| `memory/heap.rs:17` `ALLOCATOR` | talc heap (`Talck<spin::Mutex<()>>`) | yes (alloc in any ctx) | SAFE-AS-IS |
| `memory/mapper.rs:13` `MAPPER` | page tables | no | SAFE-AS-IS (lock order MAPPER→FRAMES documented) |
| `net/mod.rs:36` `NET` | smoltcp ifaces/sockets | no (driven by net_poll_task + host fns); wrapped in `without_interrupts` | SAFE-AS-IS |
| `net/sockets.rs:43` `POOL.inner` | socket pool | no | SAFE-AS-IS |
| `pci/mod.rs:25` `PCI.devices` | enumerated PCI devices (init then read) | no | SAFE-AS-IS |
| `pipe/mod.rs` `PipeInner` (Arc<Mutex>) | pipe buffer | no; wrapped in `without_interrupts` | SAFE-AS-IS |
| `proc.rs:21` `REGISTRY` | process table | no | SAFE-AS-IS |
| `pty/mod.rs:12` `PAIRS` | 4 PTY pairs | **yes** — `master_input_push` called from keyboard ISR; task sides use `without_interrupts(\|\| lock())` | SAFE-AS-IS |
| `rng.rs:8` `RNG` | ChaCha20 CSPRNG | no | SAFE-AS-IS |
| `serial.rs:31` `SERIAL` | COM1 | yes (panic/log) | SAFE-AS-IS |
| `service/mod.rs:118,123,124` `REGISTRY`,`SERVICE_QUEUE.*` | service registry + queue | no | SAFE-AS-IS |
| `ssh/server.rs:16` `SESSION_CTX` | cached host/auth keys | no | SAFE-AS-IS |
| `vfs/fat32.rs:295` `Inner` (Arc<Mutex>) | fat32 fs state | no | SAFE-AS-IS |
| `vfs/fd.rs:8` `FDS` | fd table | no | SAFE-AS-IS |
| `vfs/mod.rs:27` `MOUNTS` | mount table | no | SAFE-AS-IS |
| `vfs/tmpfs.rs` `TmpInode` (Arc<Mutex>) | tmpfs inodes | no | SAFE-AS-IS |
| `wasm/exec_queue.rs:30,36,38` `EXEC_QUEUE.*` | exec slot + wakers | no | SAFE-AS-IS |
| `wasm/pipeline.rs:31,34,35` `PIPELINE.*` | pipeline req + wakers | no | SAFE-AS-IS |

All `Mutex` here resolve to `spin::Mutex` (the project's default `Mutex` import).
The `without_interrupts(|| mutex.lock())` sites in `net/*`, `pty/*`, `pipe/*`,
`vfs/devices.rs`, `boot/banner.rs`, `boot/log.rs`, `kprint.rs`, `executor/mod.rs`
(`console_drain_task`) were each verified to wrap a `spin::Mutex.lock()` — they
are SAFE-AS-IS.

## Raw-pointer `Sync` asserts — single-core-by-design (documented)

| site | invariant | verdict |
|---|---|---|
| `executor/mod.rs:33` `unsafe impl Sync for ExecCell` | exactly one core (BSP) calls `run()`; cooperative executor is single-core by the 2026 pivot. Run-queue not yet SMP-safe — future SMP phase revisits. Comment expanded in Task 5. | SAFE (single-core invariant) |
| `cpu/mod.rs:31` `unsafe impl Sync for PerCpuArray` | per-core partition, documented | SAFE |
| `wasm/exec_queue.rs:25` `unsafe impl Send/Sync for ExecSlot` | accessed only while shell fiber is suspended waiting on `done`; raw `exit_code` ptr stable | SAFE (single-core invariant) |

## Atomic statics — all SAFE-ATOMIC

| site | use | ordering | verdict |
|---|---|---|---|
| `boot/clock.rs:14,15` `BOOT_TSC`,`TSC_PER_MS` | TSC calib | init-once write, Relaxed reads | SAFE (init-once) |
| `boot/log.rs:21` `CONSOLE_LEVEL` | fb loglevel gate | Relaxed | SAFE-ATOMIC (independent flag) |
| `console/fb.rs:47-49` `FB_VIRT`,`FB_PITCH`,`FB_BPP` | fb geometry | Release publish / Acquire load | SAFE-ATOMIC |
| `console/fb.rs:50,51` `CURSOR_POS`,`BLINK_COUNTER` | cursor (timer ISR + console) | CURSOR_POS Release/Acquire; BLINK_COUNTER Relaxed counter | SAFE-ATOMIC |
| `executor/delay.rs:46` `GEN_COUNTER` | ABA generation tag | Relaxed monotonic (uniqueness only) | SAFE-ATOMIC |
| `executor/mod.rs:24` `WAKE_PENDING` | wake flag (`__pender` from ISR ↔ run loop) | SeqCst both sides | SAFE-ATOMIC (correct — strongest ordering, missed-wake race handled by sti/hlt) |
| `keyboard/mod.rs:100,103,104,105` `EXTENDED`,`SHIFT_DOWN`,`CTRL_DOWN`,`CAPS_LOCK` | modifier state — read/written ONLY inside keyboard ISR (single core) | SeqCst | SAFE-ATOMIC (ISR-local) |
| `proc.rs:22` `NEXT_PID` | PID allocator | Relaxed fetch_add (monotonic uniqueness) | SAFE-ATOMIC |
| `pty/mod.rs:90,99,106` `CLAIMED`,`SHUTDOWN`,`LAST_ACTIVITY` | per-pair claim CAS / shutdown / activity ts | CLAIMED CAS SeqCst; SHUTDOWN/LAST_ACTIVITY Relaxed | SAFE-ATOMIC (CAS gives the claim mutual-exclusion; the Relaxed flags are independent, no cross-var ordering dependency) |
| `timer.rs:9` `TICKS` | tick counter (ISR write, task read) | Relaxed fetch_add / load | SAFE-ATOMIC (monotonic counter) |
| `wasm/exec_queue.rs` `done`,`result` (AtomicBool/I32) | exec completion signal | SeqCst | SAFE-ATOMIC |

## Conversions performed

**None.** Zero MUST-FIX sites. The codebase already uses `spin::Mutex` for all
cross-core-shared mutable state, atomics with correct ordering for lock-free
state, and documented init-once / per-core invariants for the `static mut` and
raw-pointer-`Sync` sites. Inventing conversions would only add harmless extra
locks; per the task spec the honest result is "all sites already SMP-safe".

## Self-review notes (sites considered carefully)

- **`pty/mod.rs` SHUTDOWN/LAST_ACTIVITY `Relaxed`**: classified SAFE-ATOMIC.
  These are independent boolean/timestamp flags read by the watchdog and the
  slave reader; there is no happens-before requirement tying them to OTHER
  shared state, so Relaxed is correct. The `CLAIMED` CAS that actually gates
  mutual exclusion uses SeqCst. If a future change made shutdown ordering
  observable relative to buffer state, this would warrant Acquire/Release — noted
  for future reviewers, not a current bug.
- **`keyboard/mod.rs` modifier atomics**: written AND read only inside the
  keyboard ISR, which (with a single IOAPIC redirection entry) fires on exactly
  one core. They are effectively ISR-local; atomics are belt-and-suspenders. SAFE.
- **`memory/mapper.rs:65` `&'static mut PageTable`**: matched by the `static mut`
  grep but is a function-local `let` binding, the documented sole `&mut` walker;
  all real mutation is serialized by `MAPPER` (`spin::Mutex`). SAFE.
- **`executor` run-queue (`ExecCell`)**: the one genuinely single-core-only
  structure. Not converted (would require a real SMP-safe executor — out of
  scope for this phase); instead the invariant is documented explicitly so a
  future SMP phase knows this is the thing to revisit before starting an AP that
  calls `poll()`.

## VirtualBox / GS-base caveat (post-fix, commit df1791a)

The original `this_cpu()` read `gs:[0]`, with `init_bsp` "verifying" the GS base
via an MSR read-back. This faulted (`#PF cr2=0x0`) on **VirtualBox**: VBox
accepts `wrmsr IA32_GS_BASE` into the MSR (so `GsBase::read()` returns the
written value) but does **not** update the hidden GS segment base used by
`gs:`-relative accesses. The read-back matched, the guard passed, and
`mov gs:[0]` still dereferenced linear address 0.

**Fix:** Fase 0 never uses `gs:[0]`. `this_cpu()` returns `&PER_CPU[0]`
unconditionally — correct because there is exactly one CPU (BSP = slot 0). The
release binary contains **zero `%gs:` memory accesses** (objdump-verified).
`init_bsp` still installs the GS base + records a best-effort `gs_usable()`
hint; `this_cpu_via_gs()` (dead_code) exists for later use. Booted on real
VirtualBox 7.2.8 + QEMU.

**Mandatory for Fase 1 (AP bring-up):** an MSR read-back is NOT proof that
`gs:`-relative access works. Before any per-core code uses `this_cpu_via_gs()`,
the AP path MUST do a **fault-guarded real memory probe** of `gs:[0]`
(recoverable #PF, attempt the access, confirm the value). On VMMs that don't
honor gs-base, use an APIC-ID → dense-index lookup for the core id instead.
