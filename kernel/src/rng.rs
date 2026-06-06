//! CSPRNG: ChaCha20, one per core, each seeded from a distinct RDRAND draw.
//! CLAUDE.md: never seed from the timer.
//! RDRAND absent → fatal (no entropy fallback).
//!
//! Each core has its own `RNG[cpu_id]` slot (a `Mutex<Option<ChaCha20Rng>>`).
//! All MAX_CPUS slots are seeded ONCE on the BSP at `init()` with FRESH RDRAND
//! draws → distinct streams. `fill`/`next_u64` index `RNG[cpu_id()]` and
//! therefore never contend across cores. The `spin::Mutex` inside each slot
//! guards against the rare same-core re-entrancy (RNG is not called from ISRs).

use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::{RngCore, SeedableRng};
use spin::Mutex;
use crate::cpu::MAX_CPUS;
use core::sync::atomic::{AtomicBool, Ordering};

struct RngSlot(Mutex<Option<ChaCha20Rng>>);
// SAFETY: each core touches ONLY its own slot; the Mutex serialises any
// same-core re-entrancy. No slot is shared across cores.
unsafe impl Sync for RngSlot {}

#[allow(clippy::declare_interior_mutable_const)]
const EMPTY: RngSlot = RngSlot(Mutex::new(None));
static RNG: [RngSlot; MAX_CPUS] = [EMPTY; MAX_CPUS];

/// Set once by `init()`; makes init() idempotent (boot-checks may call early).
static SEEDED: AtomicBool = AtomicBool::new(false);

pub fn rdrand_u64() -> u64 {
    use core::arch::x86_64::_rdrand64_step;
    for _ in 0..10 {
        let mut x: u64 = 0;
        // SAFETY: RDRAND availability is checked in `init` before any call here.
        if unsafe { _rdrand64_step(&mut x) } == 1 {
            return x;
        }
    }
    panic!("rng: RDRAND failed to produce entropy after 10 retries");
}

pub fn has_rdrand() -> bool {
    use core::arch::x86_64::__cpuid;
    // CPUID.01H:ECX.RDRAND[bit 30].
    // SAFETY: CPUID leaf 1 is always valid on x86-64.
    let leaf = unsafe { __cpuid(1) };
    (leaf.ecx >> 30) & 1 == 1
}

/// Seed ALL MAX_CPUS slots on the BSP from distinct RDRAND draws.
/// Idempotent (SEEDED flag). Must be called on the BSP before any AP uses RNG.
pub fn init() {
    if SEEDED.load(Ordering::SeqCst) { return; }
    if !has_rdrand() {
        panic!("rng: CPU lacks RDRAND — no secure entropy source (CLAUDE.md forbids timer seeding)");
    }
    for slot in RNG.iter() {
        let mut seed = [0u8; 32];
        for chunk in seed.chunks_mut(8) {
            chunk.copy_from_slice(&rdrand_u64().to_le_bytes());
        }
        *slot.0.lock() = Some(ChaCha20Rng::from_seed(seed));
        // Scrub the seed off the stack — crypto hygiene.
        for b in seed.iter_mut() {
            unsafe { core::ptr::write_volatile(b, 0) };
        }
    }
    SEEDED.store(true, Ordering::SeqCst);
    crate::binfo!("rng", "chacha20 seeded per-core (rdrand) cores={}", MAX_CPUS);
}

/// Fill `buf` with random bytes using THIS core's ChaCha20 stream.
/// No cross-core lock; each core touches only `RNG[cpu_id()]`.
pub fn fill(buf: &mut [u8]) {
    RNG[crate::cpu::cpu_id() as usize]
        .0.lock()
        .as_mut()
        .expect("rng: not initialized")
        .fill_bytes(buf);
}

/// Draw one random u64 from THIS core's ChaCha20 stream.
pub fn next_u64() -> u64 {
    RNG[crate::cpu::cpu_id() as usize]
        .0.lock()
        .as_mut()
        .expect("rng: not initialized")
        .next_u64()
}
