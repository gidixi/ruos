//! Prototipo B: talc per-core su sub-span disgiunti + remote-free queue
//! (feature alloc-percore-talc). THROWAWAY spike. NON di produzione.
//!
//! Limite noto (accettabile per uno spike di misura): talc::dealloc richiede la
//! Layout ESATTA usata in alloc. La remote-free queue salva solo (ptr, size) e
//! ricostruisce la Layout con align=16. È vero per la maggior parte delle alloc
//! del kernel; alloc con align>16 liberate cross-core sono rare. Se emergono fault,
//! salvare anche l'align reale. Lo scopo dello spike è il dato di contesa.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use talc::{ErrOnOom, Span, Talc, Talck};
use alloc::collections::VecDeque;
use crate::cpu::{cpu_id, MAX_CPUS};
use crate::sync::IrqMutex;

const LARGE_THRESHOLD: usize = 64 * 1024;   // ≥64 KiB → fallback globale

struct Arena {
    talc: Talck<spin::Mutex<()>, ErrOnOom>,
    base: usize,
    end: usize,     // [base, end)
}
impl Arena {
    const fn new() -> Self {
        Self { talc: Talc::new(ErrOnOom).lock(), base: 0, end: 0 }
    }
}

pub struct PerCoreTalc {
    arenas: UnsafeCell<[Arena; MAX_CPUS]>,
    fallback: Talck<spin::Mutex<()>, ErrOnOom>,
    remote: [IrqMutex<VecDeque<(usize, usize)>>; MAX_CPUS],  // (ptr, size) per owner
}
unsafe impl Sync for PerCoreTalc {}

impl PerCoreTalc {
    pub const fn new() -> Self {
        const A: Arena = Arena::new();
        const Q: IrqMutex<VecDeque<(usize, usize)>> = IrqMutex::new(VecDeque::new());
        Self {
            arenas: UnsafeCell::new([A; MAX_CPUS]),
            fallback: Talc::new(ErrOnOom).lock(),
            remote: [Q; MAX_CPUS],
        }
    }

    /// Dividi lo heap: metà ai sub-arena per-core (MAX_CPUS fette), metà al fallback
    /// (per le big alloc, es. gui.cwasm ~10 MiB, che NON entrano in una fetta).
    pub unsafe fn claim(&self, base: *mut u8, size: usize) -> Result<(), ()> {
        let per_core_total = size / 2;
        let slice = per_core_total / MAX_CPUS;
        let arenas = &mut *self.arenas.get();
        for (i, a) in arenas.iter_mut().enumerate() {
            let b = base as usize + i * slice;
            a.base = b;
            a.end = b + slice;
            a.talc.lock().claim(Span::from_base_size(b as *mut u8, slice)).map_err(|_| ())?;
        }
        let fb = base as usize + per_core_total;
        let fb_size = size - per_core_total;
        self.fallback.lock().claim(Span::from_base_size(fb as *mut u8, fb_size)).map_err(|_| ())?;
        Ok(())
    }

    /// Quale arena possiede `ptr`? Some(i) per-core, None = fallback.
    unsafe fn owner_of(&self, ptr: usize) -> Option<usize> {
        let arenas = &*self.arenas.get();
        for (i, a) in arenas.iter().enumerate() {
            if ptr >= a.base && ptr < a.end { return Some(i); }
        }
        None
    }

    /// Drena i blocchi liberati da altri core che appartengono alla MIA arena.
    /// IMPORTANTE: il lock remote[me] è rilasciato PRIMA di chiamare talc.dealloc
    /// (il blocco interno `{ ... }` droppa la guard prima di dealloc).
    unsafe fn drain_remote(&self, me: usize) {
        while let Some((ptr, size)) = { let mut q = self.remote[me].lock(); q.pop_front() } {
            let layout = Layout::from_size_align_unchecked(size, 16);
            (*self.arenas.get())[me].talc.dealloc(ptr as *mut u8, layout);
        }
    }
}

unsafe impl GlobalAlloc for PerCoreTalc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let me = cpu_id() as usize;
        self.drain_remote(me);
        if layout.size() < LARGE_THRESHOLD {
            let p = (*self.arenas.get())[me].talc.alloc(layout);
            if !p.is_null() { return p; }
        }
        self.fallback.alloc(layout)
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        let addr = p as usize;
        match self.owner_of(addr) {
            Some(owner) => {
                let me = cpu_id() as usize;
                if owner == me {
                    (*self.arenas.get())[me].talc.dealloc(p, layout);
                } else {
                    self.remote[owner].lock().push_back((addr, layout.size()));
                }
            }
            None => self.fallback.dealloc(p, layout),
        }
    }
}
