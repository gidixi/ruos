//! Cross-core TLB shootdown. The shared MAPPER (one PML4) is mutated by one core at a
//! time (serialized by MAPPER's spin::Mutex). After a mutation that can leave a stale
//! TLB entry on another core (unmap / restrict-permissions), the mutator broadcasts a
//! VEC_TLB_SHOOTDOWN IPI and waits for every other online core to invalidate the page.
//!
//! Shootdowns are RANGE-based: the mutator publishes `(addr, len)` and each handler
//! either invlpg-loops over the range (small) or reloads CR3 (large) — one broadcast
//! per range instead of one per page (see 2026-06-10-tlb-shootdown-batch-design.md).
//!
//! No extra lock: the MAPPER lock the mutator holds serializes shootdowns, so the
//! SHOOT_* statics have a single writer at a time. MAPPER MUST remain a spin::Mutex
//! (IRQs enabled while contended) so a core blocked on it still services the IPI —
//! converting MAPPER to IrqMutex would deadlock this (it masks IRQs → the waiting
//! core never sees the shootdown IPI → mutator waits forever for that core's ack).

use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};

/// Base virtual address of the range each handler core must invalidate.
static SHOOT_ADDR: AtomicU64 = AtomicU64::new(0);
/// Number of 4 KiB pages to invalidate starting at SHOOT_ADDR. Init 1 so a
/// spurious IPI (never sent, but defensively) behaves like the old single-page
/// handler. Published together with SHOOT_ADDR before the IPI (one shootdown in
/// flight at a time — MAPPER held — so the pair is always read consistent).
/// NB: the pair's consistency assumes the timeout-bail never fires: after a
/// TIMEOUT (see `shootdown_range`) a straggler handler can read a mismatched
/// (addr, len) pair — the same hazard the old single-addr code already had.
static SHOOT_LEN: AtomicU64 = AtomicU64::new(1);
/// Count of cores that have acknowledged this shootdown.
static SHOOT_ACK:  AtomicU32 = AtomicU32::new(0);
/// Count of cores that must ack (set by the mutator before sending the IPI).
static SHOOT_NEED: AtomicU32 = AtomicU32::new(0);

/// Above this many pages a handler core reloads CR3 (full non-global TLB flush)
/// instead of invlpg-looping the range — cheaper than thousands of invlpg.
const FLUSH_THRESHOLD: u64 = 32;

/// Telemetry: total `shootdown_range` calls (single-page ones included), counted
/// even on single-core boots where the broadcast itself is skipped.
static SHOOTDOWNS: AtomicU64 = AtomicU64::new(0);
/// Telemetry: shootdowns whose range exceeded FLUSH_THRESHOLD (remote cores did
/// a full CR3-reload flush instead of an invlpg loop).
static FULL_FLUSHES: AtomicU64 = AtomicU64::new(0);

/// Telemetry snapshot: `(shootdowns, full_flushes)`. Logged one-shot by the wm
/// at compositor-ready (`run()` entry) and read by the `boot-checks` range
/// boot-check to assert the batch fix (see the batch-design spec).
pub fn stats() -> (u64, u64) {
    (SHOOTDOWNS.load(Ordering::SeqCst), FULL_FLUSHES.load(Ordering::SeqCst))
}

/// Invalidate the single page at `virt` on all OTHER online cores. Thin wrapper
/// for the existing single-page call sites (unmap_page / set_flags).
pub fn shootdown(virt: u64) {
    shootdown_range(virt, 1);
}

/// Invalidate `pages` pages starting at `virt` on all OTHER online cores with ONE
/// broadcast IPI. Call AFTER the local flush, while still holding the MAPPER lock.
/// No-op when only one effective core is online.
///
/// **Single shootdown in flight:** every caller holds MAPPER, so shootdowns are
/// serialized — SHOOT_ADDR/SHOOT_LEN have exactly one writer at a time and every
/// handler reads a consistent (addr, len) pair. Do NOT call this without MAPPER held.
///
/// **Deadlock safety:** the caller holds MAPPER (spin::Mutex, IRQs enabled). A core
/// blocked on MAPPER.lock() spins with IRQs enabled → it will service the shootdown
/// IPI, run `on_ipi()`, ack, and then eventually acquire MAPPER. A core in `hlt` is
/// woken by the IPI immediately. Short IRQ-disabled sections (IrqMutex guards) ack as
/// soon as they re-enable.
pub fn shootdown_range(virt: u64, pages: usize) {
    if pages == 0 {
        return; // empty range: nothing to invalidate anywhere
    }
    SHOOTDOWNS.fetch_add(1, Ordering::SeqCst);

    // cpus_online() counts APs only; total effective cores = 1 (BSP) + APs.
    let total = 1 + crate::cpu::cpus_online();
    if total < 2 {
        return; // single-core: local flush (already done by caller) is sufficient
    }
    let need = total - 1; // all cores except ourselves

    if pages as u64 > FLUSH_THRESHOLD {
        FULL_FLUSHES.fetch_add(1, Ordering::SeqCst);
    }

    // Publish address + length, then reset ack counter, then arm need — all SeqCst
    // so every handler core sees the range before it sees need > 0.
    SHOOT_ADDR.store(virt, Ordering::SeqCst);
    SHOOT_LEN.store(pages as u64, Ordering::SeqCst);
    SHOOT_ACK.store(0, Ordering::SeqCst);
    SHOOT_NEED.store(need, Ordering::SeqCst);

    // Broadcast IPI to all other cores (APIC shorthand: all-excluding-self).
    crate::apic::lapic::send_ipi_all_but_self(crate::idt::VEC_TLB_SHOOTDOWN);

    // Bounded spin-wait for all acks. IRQs remain enabled during this wait so
    // THIS core can also service unrelated IPIs (e.g. timer). The bound is
    // generous (~2 billion iterations ≈ several seconds on a 1 GHz guest); a
    // real timeout here is a hard kernel bug (a core didn't ack), not a
    // transient: log it loudly instead of hanging forever.
    let mut spins: u64 = 0;
    while SHOOT_ACK.load(Ordering::SeqCst) < need {
        core::hint::spin_loop();
        spins += 1;
        if spins > 2_000_000_000 {
            crate::bwarn!(
                "tlb",
                "shootdown TIMEOUT addr=0x{:X} pages={} ack={}/{}",
                virt,
                pages,
                SHOOT_ACK.load(Ordering::SeqCst),
                need
            );
            return;
        }
    }
}

/// IPI handler body — called from `idt::tlb_shootdown_handler`. IRQ context:
/// no locks, no allocation. Invalidates the published range in this core's TLB
/// and increments the ack counter.
#[inline]
pub fn on_ipi() {
    let addr = SHOOT_ADDR.load(Ordering::SeqCst);
    let len  = SHOOT_LEN.load(Ordering::SeqCst);
    if len <= FLUSH_THRESHOLD {
        // Small range: per-page invlpg, cheaper than nuking the whole TLB.
        // SAFETY: `invlpg` is always safe to execute; it only invalidates a TLB
        // entry on the current core, never touches memory at the address.
        for i in 0..len {
            let a = addr.wrapping_add(i * 0x1000);
            unsafe {
                core::arch::asm!(
                    "invlpg [{a}]",
                    a = in(reg) a,
                    options(nostack, preserves_flags)
                );
            }
        }
    } else {
        // Large range: full TLB flush by reloading CR3. A CR3 reload does NOT
        // flush GLOBAL pages, but no mapping the kernel installs ever sets
        // PageTableFlags::GLOBAL (mapper::map_page callers, platform.rs and
        // demand.rs `prot_to_flags` only use PRESENT/WRITABLE/NO_EXECUTE/
        // WRITE_THROUGH/NO_CACHE — verified by grep over kernel/src). In
        // particular every page of the Wasmtime VA window is non-global, so the
        // reload is guaranteed to drop any stale entry in the shootdown range.
        // SAFETY: rewriting CR3 with its current value is always valid; it only
        // flushes this core's TLB (no PCID in use).
        use x86_64::registers::control::Cr3;
        let (frame, flags) = Cr3::read();
        unsafe { Cr3::write(frame, flags); }
    }
    SHOOT_ACK.fetch_add(1, Ordering::SeqCst);
}
