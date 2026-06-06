//! Cross-core TLB shootdown. The shared MAPPER (one PML4) is mutated by one core at a
//! time (serialized by MAPPER's spin::Mutex). After a mutation that can leave a stale
//! TLB entry on another core (unmap / restrict-permissions), the mutator broadcasts a
//! VEC_TLB_SHOOTDOWN IPI and waits for every other online core to invalidate the page.
//!
//! No extra lock: the MAPPER lock the mutator holds serializes shootdowns, so the
//! SHOOT_* statics have a single writer at a time. MAPPER MUST remain a spin::Mutex
//! (IRQs enabled while contended) so a core blocked on it still services the IPI —
//! converting MAPPER to IrqMutex would deadlock this (it masks IRQs → the waiting
//! core never sees the shootdown IPI → mutator waits forever for that core's ack).

use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};

/// Virtual address to invlpg on each handler core.
static SHOOT_ADDR: AtomicU64 = AtomicU64::new(0);
/// Count of cores that have acknowledged this shootdown.
static SHOOT_ACK:  AtomicU32 = AtomicU32::new(0);
/// Count of cores that must ack (set by the mutator before sending the IPI).
static SHOOT_NEED: AtomicU32 = AtomicU32::new(0);

/// Invalidate `virt` on all OTHER online cores. Call AFTER the local flush, while
/// still holding the MAPPER lock. No-op when only one effective core is online.
///
/// **Deadlock safety:** the caller holds MAPPER (spin::Mutex, IRQs enabled). A core
/// blocked on MAPPER.lock() spins with IRQs enabled → it will service the shootdown
/// IPI, run `on_ipi()`, ack, and then eventually acquire MAPPER. A core in `hlt` is
/// woken by the IPI immediately. Short IRQ-disabled sections (IrqMutex guards) ack as
/// soon as they re-enable. MAPPER serializes shootdowns: only one mutator holds MAPPER
/// at a time, so SHOOT_* have exactly one writer.
pub fn shootdown(virt: u64) {
    // cpus_online() counts APs only; total effective cores = 1 (BSP) + APs.
    let total = 1 + crate::cpu::cpus_online();
    if total < 2 {
        return; // single-core: local flush (already done by caller) is sufficient
    }
    let need = total - 1; // all cores except ourselves

    // Publish the address, then reset ack counter, then arm need — all SeqCst
    // so every handler core sees the address before it sees need > 0.
    SHOOT_ADDR.store(virt, Ordering::SeqCst);
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
                "shootdown TIMEOUT addr=0x{:X} ack={}/{}",
                virt,
                SHOOT_ACK.load(Ordering::SeqCst),
                need
            );
            return;
        }
    }
}

/// IPI handler body — called from `idt::tlb_shootdown_handler`.
/// Invalidates the shootdown address in this core's TLB and increments the ack counter.
#[inline]
pub fn on_ipi() {
    let addr = SHOOT_ADDR.load(Ordering::SeqCst);
    // SAFETY: `invlpg` is always safe to execute; it only invalidates a TLB
    // entry on the current core, never touches memory at `addr`.
    unsafe {
        core::arch::asm!(
            "invlpg [{addr}]",
            addr = in(reg) addr,
            options(nostack, preserves_flags)
        );
    }
    SHOOT_ACK.fetch_add(1, Ordering::SeqCst);
}
