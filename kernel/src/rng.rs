//! CSPRNG: ChaCha20 seeded from RDRAND. CLAUDE.md: never seed from the timer.
//! RDRAND absent → fatal (no entropy fallback).

use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::{RngCore, SeedableRng};
use spin::Mutex;

static RNG: Mutex<Option<ChaCha20Rng>> = Mutex::new(None);

fn rdrand_u64() -> u64 {
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

fn has_rdrand() -> bool {
    use core::arch::x86_64::__cpuid;
    // CPUID.01H:ECX.RDRAND[bit 30].
    // SAFETY: CPUID leaf 1 is always valid on x86-64.
    let leaf = unsafe { __cpuid(1) };
    (leaf.ecx >> 30) & 1 == 1
}

pub fn init() {
    if !has_rdrand() {
        panic!("rng: CPU lacks RDRAND — no secure entropy source (CLAUDE.md forbids timer seeding)");
    }
    let mut seed = [0u8; 32];
    for chunk in seed.chunks_mut(8) {
        chunk.copy_from_slice(&rdrand_u64().to_le_bytes());
    }
    *RNG.lock() = Some(ChaCha20Rng::from_seed(seed));
    // Scrub the seed off the stack — crypto hygiene for the entropy material
    // (matters once SSH session keys derive from this RNG). volatile so the
    // optimizer can't elide the dead store.
    for b in seed.iter_mut() {
        unsafe { core::ptr::write_volatile(b, 0) };
    }
    crate::binfo!("rng", "chacha20 seeded (rdrand)");
}

pub fn fill(buf: &mut [u8]) {
    let mut g = RNG.lock();
    g.as_mut().expect("rng: not initialized").fill_bytes(buf);
}

pub fn next_u64() -> u64 {
    let mut g = RNG.lock();
    g.as_mut().expect("rng: not initialized").next_u64()
}
