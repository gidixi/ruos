# Step 3d — Cross-core TLB shootdown Implementation Plan

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development / executing-plans. `- [ ]`.

**Goal:** When one core mutates the single shared page table (`MAPPER`, one PML4) in a
way that can leave a STALE entry in another core's TLB — i.e. **unmap** (present→absent)
or **set_flags** (restrict permissions, e.g. W→RO+X for W^X) — broadcast a TLB-shootdown
IPI so every other online core invalidates the affected page before the mutator proceeds.
Without this, a core using a stale TLB entry reads/writes a page that has been
unmapped/re-permissioned = **silent memory-safety corruption**. Spec §13.1 (gap #1).
The hardest correctness piece of the migration — implement carefully.

**Why now:** the GUI pin (Step 5) doesn't need it (compositor instantiated on the BSP,
no steady-state MAPPER mutation on the GUI core). But it is the **hard prerequisite** for
running dynamic WASM apps on the AP `ComputeApp` cores (they load/teardown modules + grow
linear memory + flip W^X → MAPPER mutations whose stale TLBs on other cores would corrupt).

**Prerequisites (committed):** Step 2 (`VEC_TLB_SHOOTDOWN=0x42` reserved; targeted/
broadcast IPI; `cpu_id`/`cpus_online`/`lapic_id_of`), 3b (per-core executors so other
cores genuinely run + cache TLB entries).

**CHANGELOG:** next free on this branch. Trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

## Design

Current (`memory/mapper.rs`): `map_page`/`unmap_page`/`set_flags` each lock `MAPPER`
(plain `spin::Mutex`), mutate, and `.flush()` = LOCAL `invlpg` (current core only).

Add a shootdown after the local flush, for **unmap_page + set_flags only** (NOT
map_page — a new present mapping over a previously not-present page needs no shootdown:
x86 does not cache not-present entries, a stale negative access just faults + re-walks).

**Shootdown protocol (serialized by the MAPPER lock — only one in flight):**
1. The mutator, still holding `MAPPER`, after its local `invlpg`:
   - if `cpus_online() < 2` → nothing to do (single effective core; return).
   - else: `SHOOT_ADDR.store(virt, SeqCst)`; `SHOOT_ACK.store(0, SeqCst)`; pick the set
     of OTHER online cores; `SHOOT_PENDING.store(n_others, SeqCst)`; send
     `VEC_TLB_SHOOTDOWN` IPI to all-but-self (`send_ipi_all_but_self`); spin-wait
     (bounded, IRQs ENABLED) until `SHOOT_ACK == n_others`; then return (release MAPPER).
2. Handler `tlb_shootdown_handler` (each other core): `invlpg`(SHOOT_ADDR);
   `SHOOT_ACK.fetch_add(1, SeqCst)`; `eoi()`.

**Deadlock analysis (load-bearing — `MAPPER` MUST stay a plain `spin::Mutex`):** a core
waiting on `MAPPER.lock()` spins with IRQs ENABLED (spin::Mutex doesn't mask), so it
SERVICES the shootdown IPI while waiting → acks → the mutator proceeds → releases MAPPER.
A halted core (`enable_and_hlt`) is woken by the IPI and its handler runs. A core in a
short IRQ-disabled section (IrqMutex guard) acks as soon as it re-enables. The MAPPER lock
serializes shootdowns (one mutator at a time) so `SHOOT_*` statics need no extra lock.
⚠️ Do NOT convert MAPPER to IrqMutex — that would mask the shootdown IPI on a waiting core
→ deadlock. Keep `spin::Mutex`.

**Bounded wait:** the ack spin must have a large bound (e.g. 1e9 iters) and, on timeout,
log `tlb shootdown TIMEOUT` (a core didn't ack — bug) rather than hang forever. A timeout
is a hard failure to investigate, not a silent skip.

---

## File Structure
- `kernel/src/memory/tlb.rs` — NEW. `SHOOT_ADDR`/`SHOOT_ACK`/`SHOOT_PENDING`,
  `shootdown(virt)`, `tlb_shootdown_handler` registration helper.
- `kernel/src/memory/mod.rs` — `pub mod tlb;`.
- `kernel/src/idt.rs` — register `VEC_TLB_SHOOTDOWN` → handler (calls `tlb::on_ipi()`).
- `kernel/src/memory/mapper.rs` — call `tlb::shootdown(virt)` after the local flush in
  `unmap_page` and `set_flags` (NOT map_page).
- `kernel/src/boot/phases/interrupts.rs` — the no-fault remap test.
- `CHANGELOG/NN`.

---

## Task 1: TLB shootdown mechanism + IPI handler

**Files:** `kernel/src/memory/tlb.rs` (new), `kernel/src/memory/mod.rs`, `kernel/src/idt.rs`

- [ ] **Step 1: `memory/tlb.rs`** —
```rust
//! Cross-core TLB shootdown. The shared MAPPER (one PML4) is mutated by one core at a
//! time (serialized by MAPPER's spin::Mutex). After a mutation that can leave a stale
//! TLB entry on another core (unmap / restrict-permissions), the mutator broadcasts a
//! VEC_TLB_SHOOTDOWN IPI and waits for every other online core to invalidate the page.
//!
//! No extra lock: the MAPPER lock the mutator holds serializes shootdowns, so the
//! SHOOT_* statics have a single writer at a time. MAPPER MUST remain a spin::Mutex
//! (IRQs enabled while contended) so a core blocked on it still services the IPI —
//! converting MAPPER to IrqMutex would deadlock this.

use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};

static SHOOT_ADDR: AtomicU64 = AtomicU64::new(0);   // virt to invlpg
static SHOOT_ACK:  AtomicU32 = AtomicU32::new(0);   // cores that have acked
static SHOOT_NEED: AtomicU32 = AtomicU32::new(0);   // cores that must ack

/// Invalidate `virt` on all OTHER online cores. Call AFTER the local flush, while
/// holding the MAPPER lock. No-op on a single effective core.
pub fn shootdown(virt: u64) {
    // cpus_online() counts APs only; total cores = 1 (BSP) + APs.
    let total = 1 + crate::cpu::cpus_online();
    if total < 2 { return; }
    let need = total - 1; // all but self
    SHOOT_ADDR.store(virt, Ordering::SeqCst);
    SHOOT_ACK.store(0, Ordering::SeqCst);
    SHOOT_NEED.store(need, Ordering::SeqCst);
    crate::apic::lapic::send_ipi_all_but_self(crate::idt::VEC_TLB_SHOOTDOWN);
    // Bounded wait for all acks (IRQs enabled — we don't mask here).
    let mut spins: u64 = 0;
    while SHOOT_ACK.load(Ordering::SeqCst) < need {
        core::hint::spin_loop();
        spins += 1;
        if spins > 2_000_000_000 {
            crate::bwarn!("tlb", "shootdown TIMEOUT addr=0x{:X} ack={}/{}", virt, SHOOT_ACK.load(Ordering::SeqCst), need);
            return;
        }
    }
}

/// IPI handler body (called from idt::tlb_shootdown_handler): invalidate the page and ack.
pub fn on_ipi() {
    let addr = SHOOT_ADDR.load(Ordering::SeqCst);
    // SAFETY: invlpg is always safe; invalidates the TLB entry for `addr` on this core.
    unsafe { core::arch::asm!("invlpg [{}]", in(reg) addr, options(nostack, preserves_flags)); }
    SHOOT_ACK.fetch_add(1, Ordering::SeqCst);
}
```

- [ ] **Step 2: mod.rs** — add `pub mod tlb;`.

- [ ] **Step 3: idt.rs handler** — register `VEC_TLB_SHOOTDOWN` (0x42, already a const):
```rust
idt[VEC_TLB_SHOOTDOWN].set_handler_fn(tlb_shootdown_handler);
```
and the handler (next to wake/inbox handlers):
```rust
extern "x86-interrupt" fn tlb_shootdown_handler(_frame: InterruptStackFrame) {
    crate::memory::tlb::on_ipi();
    crate::apic::lapic::eoi();
}
```

- [ ] **Step 4: build** — `make test-boot` → `TEST_BOOT_PASS` (handler registered, nothing
  calls shootdown yet — Task 2 wires it). No behaviour change.

---

## Task 2: Hook shootdown into unmap_page + set_flags

**Files:** `kernel/src/memory/mapper.rs`

- [ ] **Step 1: unmap_page** — after `flush.flush();` (the local invlpg), add
  `crate::memory::tlb::shootdown(virt.as_u64());` (still inside the fn, MAPPER lock held).
- [ ] **Step 2: set_flags** — after the `.flush()` in the `unsafe` block, add the same
  `crate::memory::tlb::shootdown(virt.as_u64());`.
- [ ] **Step 3: map_page — do NOT add shootdown** — add a one-line comment in `map_page`
  explaining why (new present mapping over a not-present page needs no shootdown: x86
  doesn't cache not-present entries). Leave map_page's local `.flush()` as-is.
- [ ] **Step 4: build** — `make test-boot` → `TEST_BOOT_PASS`. On 1 core `shootdown`
  early-returns (total<2). The W^X self-test (set_flags) + any boot unmaps now call
  shootdown but it's a no-op on 1 core. Then `make iso CARGO_FEATURES="boot-checks"` +
  boot `-smp 4`: boot must still complete (the W^X self-test runs set_flags → shootdown
  to 3 cores during boot; confirm no hang / no `shootdown TIMEOUT`).
- [ ] **Step 5: commit** (Tasks 1+2) —
```
git add kernel/src/memory/tlb.rs kernel/src/memory/mod.rs kernel/src/idt.rs kernel/src/memory/mapper.rs
git commit -m "feat(smp): 3d — cross-core TLB shootdown on unmap/set_flags (VEC_TLB_SHOOTDOWN)"
```
Trailer as above.

---

## Task 3: GATE — no-fault remap test (proves no stale TLB)

The test must prove a remap is SEEN by another core (no stale entry) WITHOUT triggering a
fault (a #PF halts the kernel). Strategy: map a test page to frame A (sentinel 0xAAAA…),
have an AP read it (caching translation A), remap to frame B (sentinel 0xBBBB…) WITH
shootdown, have the AP read AGAIN — it must see B (flushed), not A (stale).

**Files:** `kernel/src/boot/phases/interrupts.rs`, `kernel/src/memory/*` (test helpers), `CHANGELOG/NN`

- [ ] **Step 1: test plumbing** — Pick a free test virt (e.g. `0x4444_0000_0000`,
  avoiding the existing paging-smoke virt). Allocate two frames via
  `crate::memory::allocate_frame()`. Write 0xAAAAAAAA into frame A's HHDM alias and
  0xBBBBBBBB into frame B's HHDM alias. The AP "read test_virt" op = a `fn(&[u8])->u64`
  inbox op that does `unsafe { read_volatile(test_virt as *const u32) } as u64` (runs on
  the AP, caches its TLB entry).
- [ ] **Step 2: boot-check sequence** (under `#[cfg(feature="boot-checks")]`, after
  bringup, only if `cpus_online() >= 2`; pick a ComputeApp core id, e.g. 2):
  1. `map_page(test_virt, frame_A, PRESENT|WRITABLE|NO_EXECUTE)`.
  2. inbox `request(2, read_test, ..)` → drive inline → `r1` (expect 0xAAAAAAAA; core 2
     now has the translation cached).
  3. `unmap_page(test_virt)` then `map_page(test_virt, frame_B, ...)` — the `unmap_page`
     fires the shootdown to core 2 (invalidates its cached entry).
  4. inbox `request(2, read_test, ..)` → `r2`.
  5. `crate::binfo!("tlb", "remap seen by ap: r1=0x{:X} r2=0x{:X} shootdown_ok={}", r1, r2, r1==0xAAAAAAAA && r2==0xBBBBBBBB);`
  Use the same inline-poll noop-waker pattern as the Step-2/3c boot-checks.
- [ ] **Step 3: run the gate -smp 4, TWICE** —
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso CARGO_FEATURES="boot-checks" && for i in 1 2; do echo "run $i"; timeout 90 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -m 512 -no-reboot -display none -serial stdio -device qemu-xhci -cdrom build/os.iso 2>&1 | grep -E "tlb|APs online"; done'
```
GATE: `remap seen by ap: r1=0xAAAAAAAA r2=0xBBBBBBBB shootdown_ok=true`, stable BOTH runs,
and NO `shootdown TIMEOUT`. 
- `r2=0xBBBBBBBB` ⇒ core 2 saw the NEW frame after the remap+shootdown = its stale TLB
  entry WAS invalidated = shootdown works. THE PROOF.
- `r2=0xAAAAAAAA` ⇒ core 2 used a STALE TLB entry = shootdown FAILED (didn't reach core 2
  or didn't invlpg). Do NOT mark 3d done.
- `r1!=0xAAAAAAAA` ⇒ the test setup is wrong (core 2 didn't read frame A first).
ALSO: `make test-boot` (1 core) → `TEST_BOOT_PASS` (shootdown no-op, test skipped). `make
run-smp-test`/`run-smp2-test`/`run-ssh-gui-test` → PASS (boot does set_flags W^X →
shootdown during boot; confirm no regression/hang).
- [ ] **Step 4: changelog + commit** —
```
git add kernel/src/boot/phases/interrupts.rs CHANGELOG/NN-... <any test helper files>
git commit -m "test(smp): 3d — no-fault remap proves cross-core TLB shootdown invalidates stale entries"
```
Trailer as above.

---

## Self-Review
- **Shootdown only on unmap/set_flags:** map_page (new present) needs none (x86 doesn't
  cache not-present entries). This keeps the common path (mapping) IPI-free; only the
  dangerous mutations (removing/restricting) shoot down.
- **No deadlock:** MAPPER stays `spin::Mutex` → a core blocked on it spins with IRQs
  enabled → services the shootdown IPI → acks. Halted cores wake on the IPI. MAPPER lock
  serializes shootdowns (single writer of SHOOT_* statics). The bounded ack-wait logs a
  TIMEOUT instead of hanging (a missed ack is a hard bug to find, not silently skipped).
- **Test is no-fault + decisive:** remap A→B with shootdown; the AP seeing B (not the
  stale A) is positive proof the entry was invalidated, with no #PF.
- **Boot exercises it for real:** the W^X self-test (`set_flags`) + boot unmaps fire
  shootdowns to 3 cores during the -smp 4 boot — the boot completing without TIMEOUT is
  an additional live check.
- **Risk: HIGH (subtlest bug class).** The deadlock argument hinges on MAPPER being
  spin::Mutex + the IPI being serviced by waiting/halted cores. If the gate shows
  `r2=0xAAAA` (stale) the invlpg/IPI path is broken; if it TIMEOUTs a core isn't acking
  (IRQ masked too long, or the IPI not delivered). Do NOT mark done unless r2=0xBBBB on
  both runs AND no TIMEOUT AND the goal/regression tests stay green.
