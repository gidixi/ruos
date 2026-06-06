# Step 1b — Magazine allocator to production Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Promote the spike's per-core **magazine** allocator (decided winner, see
`docs/superpowers/decisions/2026-06-05-allocator-architecture.md` §8) to the kernel's
DEFAULT `#[global_allocator]`, retire the losing prototype B (`alloc-percore-talc`),
clean the code from throwaway-spike to production quality, and validate with the FULL
test suite (the allocator touches every allocation: boot, wasm, gui, net, ssh).

**Architecture:** The magazine (`MagazineAlloc`, `kernel/src/memory/alloc_magazine.rs`)
becomes the default global allocator. Plain global talc is kept behind a `legacy-talc`
cargo feature as an escape hatch / comparison baseline. The per-core size-class cache
sits in front of a single global talc; cache hit avoids the allocator; `cpu_id()` is
now cheap (Step 1a RDTSCP, ~23 ns). The canonical-class-layout + `align>16` bypass
(correctness fixes already in the prototype) are retained.

**Tech Stack:** Rust `no_std`, `talc` 4, `spin`/`IrqMutex`, cargo features, RDTSCP
`cpu_id()` (Step 1a, committed), boot-check markers + `make` test targets.

**Prerequisite:** Step 1a (fast `cpu_id`) — DONE + committed (`eec7398`). The magazine
calls `cpu_id()` per alloc; it must be the fast RDTSCP path, which it now is.

---

## File Structure

- `kernel/Cargo.toml` — replace features `alloc-magazine` / `alloc-percore-talc` with a
  single `legacy-talc` escape hatch (default = magazine).
- `kernel/src/memory/alloc_magazine.rs` — promote to production: drop "THROWAWAY spike"
  language, document the design + invariants properly, keep canonical-layout + align>16
  bypass; (optional) tune size classes.
- `kernel/src/memory/heap.rs` — `#[global_allocator]` = magazine by default;
  `legacy-talc` feature = plain `Talck`. `init_heap` claim cfg simplified to 2 branches.
- `kernel/src/memory/mod.rs` — `alloc_magazine` always compiled (not feature-gated);
  remove `alloc_percore_talc` re-export.
- DELETE `kernel/src/memory/alloc_percore_talc.rs` (prototype B, retired).
- `CHANGELOG/NN-...` — one entry.

**Note on CHANGELOG numbering:** main has diverged and also uses 296-300. On THIS branch
the highest is 305. Use the next free number on this branch (verify: `ls CHANGELOG | sort
-n`), and accept that a future merge to main will renumber. Do NOT renumber here.

---

## Task 1: Retire prototype B + flip features to `legacy-talc`

**Files:**
- Modify: `kernel/Cargo.toml`
- Delete: `kernel/src/memory/alloc_percore_talc.rs`
- Modify: `kernel/src/memory/mod.rs`
- Modify: `kernel/src/memory/heap.rs`

- [ ] **Step 1: Cargo features** — In `[features]`, REMOVE `alloc-magazine = []` and
  `alloc-percore-talc = []`; ADD `legacy-talc = []`. (Default build now = magazine; the
  escape hatch is `--features legacy-talc`.)

- [ ] **Step 2: Delete prototype B** — `git rm kernel/src/memory/alloc_percore_talc.rs`.

- [ ] **Step 3: mod.rs** — In `kernel/src/memory/mod.rs`: change the `alloc_magazine`
  module to ALWAYS compile (remove its `#[cfg(feature = "alloc-magazine")]`); REMOVE the
  `#[cfg(feature = "alloc-percore-talc")] pub mod alloc_percore_talc;` line.

- [ ] **Step 4: heap.rs `#[global_allocator]`** — Replace the three cfg-gated branches
  with two:
```rust
#[cfg(not(feature = "legacy-talc"))]
#[global_allocator]
pub static ALLOCATOR: crate::memory::alloc_magazine::MagazineAlloc =
    crate::memory::alloc_magazine::MagazineAlloc::new();

#[cfg(feature = "legacy-talc")]
#[global_allocator]
pub static ALLOCATOR: Talck<spin::Mutex<()>, ErrOnOom> = Talc::new(ErrOnOom).lock();
```
Keep the Step-1 SMP-baseline comment above the legacy branch (it documents the talc
spinlock). The `use talc::{ErrOnOom, Span, Talc, Talck};` import must be available for
the `legacy-talc` build AND for `init_heap` (the magazine's `claim` uses `Span`
internally, so the import may be needed unconditionally now — verify it compiles both
ways; gate the import only if it causes unused-import warnings).

- [ ] **Step 5: heap.rs `init_heap` claim** — Replace the cfg'd claim with two branches:
```rust
#[cfg(not(feature = "legacy-talc"))]
unsafe { ALLOCATOR.claim(virt_base as *mut u8, HEAP_SIZE).map_err(|_| HeapInitError::ClaimFailed)?; }
#[cfg(feature = "legacy-talc")]
unsafe { ALLOCATOR.lock().claim(Span::from_base_size(virt_base as *mut u8, HEAP_SIZE)).map_err(|_| HeapInitError::ClaimFailed)?; }
```

- [ ] **Step 6: build both configs** — via WSL (LONG; 600000 ms or background):
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make test-boot'
```
Expected `TEST_BOOT_PASS` (default = magazine now). Then:
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make test-boot CARGO_FEATURES="boot-checks,legacy-talc"'
```
Expected `TEST_BOOT_PASS` (escape hatch still works). If either fails, capture the exact
error.

- [ ] **Step 7: Commit** —
```
git add kernel/Cargo.toml kernel/src/memory/mod.rs kernel/src/memory/heap.rs
git rm kernel/src/memory/alloc_percore_talc.rs
git commit -m "refactor(smp): magazine is the default allocator; retire prototype B; legacy-talc escape hatch"
```
End body with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

## Task 2: Productionize `alloc_magazine.rs`

Turn the throwaway-spike module into production code: real module docs, stated
invariants, no "THROWAWAY/spike" language. NO behavior change (the algorithm is the
proven one).

**Files:**
- Modify: `kernel/src/memory/alloc_magazine.rs`

- [ ] **Step 1: Rewrite the module doc** — Replace the throwaway header with a
  production description:
```rust
//! Per-core magazine allocator: a per-CPU size-class free-list cache in front of one
//! global talc heap. Small alloc/free (size & align fit a class, align <= 16) hit the
//! local magazine without touching talc — eliminating the global talc-lock traffic that
//! per-core executors (SMP Step 3) would otherwise serialise on. Cache miss / overflow
//! and all large or high-align allocations go to the shared talc.
//!
//! Per-core indexing uses `cpu_id()` (RDTSCP fast path, ~tens of cycles). Each core
//! touches only `mags[cpu_id]`, with interrupts disabled across the short push/pop so an
//! ISR on the same core cannot observe a half-updated free-list (no cross-core sharing
//! of a magazine).
//!
//! INVARIANTS:
//! - Every cached block in class `i` was allocated from talc with the CANONICAL layout
//!   `Layout(16<<i, 16)`, so any block handed out for a request that maps to class `i`
//!   is >= the requested size and 16-aligned; recycling never returns an undersized or
//!   misaligned block. `align > 16` and `size > MAX_SMALL` bypass the magazine entirely.
//! - talc only ever sees alloc/free at the canonical class layout (cache miss / overflow),
//!   so its metadata is always consistent. Cross-core free is trivial: the freeing core
//!   pushes to ITS magazine, or returns the block to the single global talc which owns
//!   the whole heap — no remote-free queue needed.
```

- [ ] **Step 2: Remove spike-isms in code comments** — Delete any "spike"/"throwaway"/
  "NON di produzione" lines elsewhere in the file. Keep the technical comments. Do NOT
  change `size_class`, `MagazineAlloc::alloc/dealloc`, the IF-masking, or the constants.

- [ ] **Step 3: (optional) size-class assertion** — Add a `const _: () = assert!(16 <<
  (NUM_CLASSES - 1) == MAX_SMALL);` so the class table and `MAX_SMALL` can't silently
  drift. Only if it compiles cleanly (const assert in no_std).

- [ ] **Step 4: build** —
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make test-boot'
```
Expected `TEST_BOOT_PASS`, no new warnings about the magazine module.

- [ ] **Step 5: Commit** —
```
git add kernel/src/memory/alloc_magazine.rs
git commit -m "docs(smp): magazine allocator — production docs + invariants (no behavior change)"
```
Trailer as above.

---

## Task 3: Full test-suite validation (the allocator touches everything)

The default allocator changed kernel-wide. Run the real workload tests, not just
test-boot, to prove wasm/gui/net/ssh/pipe/fuel still work on the magazine.

**Files:** none (verification only) + `CHANGELOG/NN`.

- [ ] **Step 1: boot-checks self-tests** —
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make test-boot'
```
Expected `TEST_BOOT_PASS`.

- [ ] **Step 2: full smoke battery (net/USB/AHCI/FAT/rtop)** —
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test'
```
Expected `TEST_PASS` (it asserts shell sentinel, PCI/xHCI, DHCP, AHCI, FAT, readdir,
rtop, USB). This exercises wasm tools + net + storage under the magazine.

- [ ] **Step 3: SSH + pipe + fuel** —
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-ssh-test && make run-pipe-test && make run-fuel-test'
```
Expected each test's PASS marker. (SSH = sunset stack allocs; pipe = pipeline; fuel =
wasm fuel metering — all alloc-heavy paths.)

- [ ] **Step 4: SMP tests** —
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-smp-test && make run-smp2-test'
```
Expected their PASS markers (AP compute pool under the magazine).

- [ ] **Step 5: record results + changelog** — Create the next changelog entry (verify
  number with `ls CHANGELOG | sort -n | tail -1`). Cosa: magazine promoted to default +
  prototype B retired + full-suite green. Perché: Step 1b — adopt the spike's winning
  allocator (decision §8) ahead of Step 3 (per-core executors). List which test targets
  passed (paste the PASS markers).
```
git add CHANGELOG/NN-26-06-05-smp-step1b-magazine-default.md
git commit -m "test(smp): Step 1b — full-suite validation of magazine default allocator"
```
Trailer as above.

- [ ] **Step 6: any failure** — If ANY suite fails on the magazine but passes on
  `legacy-talc` (re-run the failing target with `CARGO_FEATURES=...,legacy-talc` to
  confirm it's the allocator), STOP and report: the magazine has a real bug to fix
  before it can be default. Do not mark Step 1b done with a failing suite.

---

## Self-Review

**Spec coverage:** decision record §8 mandates "adopt Magazine A; keep canonical-class-
layout + align>16 bypass; drop B's remote-free/align=16/drain_remote; profile size-class
table." Task 1 retires B + makes magazine default; Task 2 keeps the canonical layout +
align>16, documents invariants; Task 3 validates kernel-wide. The size-class "profiling"
is left as a documented optional (Step 3 step) — the proven table stands until real
per-core executor workloads (Step 3) justify retuning.

**Placeholder scan:** `NN` in changelog filenames is resolved at execution (the highest
on this branch + 1) — not a placeholder, an instruction. No TBD/TODO.

**Type consistency:** `MagazineAlloc::new()/claim(base,size)` match heap.rs usage;
`legacy-talc` branch uses `Talck`/`Span`/`Talc`/`ErrOnOom` consistently. The retired B
(`PerCoreTalc`) is fully removed (file + feature + re-export).

**Risk:** making the magazine default is kernel-wide. Task 3's full suite is the gate;
the `legacy-talc` escape hatch lets us bisect allocator-vs-other regressions and ship a
fallback if a field issue appears.
