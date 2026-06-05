//! Prototipo A: magazine per-core davanti a UN talc globale (feature alloc-magazine).
//! THROWAWAY spike — confronto con alloc_percore_talc. NON di produzione.
//!
//! Limite noto (accettabile per uno spike di misura): la magazine ricicla i blocchi
//! per size-class. Ogni blocco in classe `idx` è allocato da talc con la Layout
//! canonica della classe (size=16<<idx, align=16). Le dealloc di overflow passano a
//! talc la stessa Layout canonica (non quella del chiamante). Il chiamante non vede
//! la differenza: il blocco restituito soddisfa la sua Layout per size e align
//! (class_size >= request.size, 16 >= request.align per costruzione di size_class).
//! La magazine è bypassata per align > 16 (→ talc diretto) e size > MAX_SMALL.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;
use talc::{ErrOnOom, Span, Talc, Talck};
use crate::cpu::{cpu_id, MAX_CPUS};

const NUM_CLASSES: usize = 8;          // 16,32,64,128,256,512,1024,2048 B
const MAX_SMALL: usize = 2048;
const CACHE_DEPTH: usize = 64;          // nodi liberi per (core, classe)

/// Canonical per-class alignment: all magazine blocks have align=16.
const CLASS_ALIGN: usize = 16;

/// Returns (class_index, class_layout) for the given layout, or None if the
/// layout bypasses the magazine (align > 16 or size > MAX_SMALL or size == 0).
/// The canonical class_layout has size == 16<<idx and align == CLASS_ALIGN.
/// SAFETY-REFINEMENT: any layout with align > 16 bypasses the magazine entirely,
/// so high-align requests always go to talc which aligns them correctly. This
/// prevents potential misaligned pointer UB from class-level recycling.
#[inline]
fn size_class(layout: Layout) -> Option<(usize, Layout)> {
    if layout.align() > CLASS_ALIGN { return None; }  // REFINEMENT: high-align → talc
    let need = layout.size().max(layout.align());
    if need == 0 || need > MAX_SMALL { return None; }
    let mut sz = 16usize;
    let mut idx = 0usize;
    while sz < need { sz <<= 1; idx += 1; }
    // SAFETY: sz is a power of two >= 16; CLASS_ALIGN is a power of two >= 1.
    let cls_layout = unsafe { Layout::from_size_align_unchecked(sz, CLASS_ALIGN) };
    Some((idx, cls_layout))
}

/// free-list intrusiva: il primo usize di ogni blocco libero è il "next".
struct Magazine {
    heads: [*mut u8; NUM_CLASSES],
    depth: [u16; NUM_CLASSES],
}
impl Magazine {
    const fn new() -> Self { Self { heads: [ptr::null_mut(); NUM_CLASSES], depth: [0; NUM_CLASSES] } }
}

struct PerCpuMag(core::cell::UnsafeCell<[Magazine; MAX_CPUS]>);
unsafe impl Sync for PerCpuMag {}   // partizionato per cpu_id, IF-mask sul push/pop

pub struct MagazineAlloc {
    inner: Talck<spin::Mutex<()>, ErrOnOom>,
    mags: PerCpuMag,
}

impl MagazineAlloc {
    pub const fn new() -> Self {
        const M: Magazine = Magazine::new();
        Self {
            inner: Talc::new(ErrOnOom).lock(),
            mags: PerCpuMag(core::cell::UnsafeCell::new([M; MAX_CPUS])),
        }
    }
    /// Claim dello span heap (chiamato da init_heap).
    pub unsafe fn claim(&self, base: *mut u8, size: usize) -> Result<(), ()> {
        self.inner.lock().claim(Span::from_base_size(base, size)).map(|_| ()).map_err(|_| ())
    }
}

unsafe impl GlobalAlloc for MagazineAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if let Some((cls, cls_layout)) = size_class(layout) {
            let saved = x86_64::instructions::interrupts::are_enabled();
            x86_64::instructions::interrupts::disable();
            let mags = &mut (*self.mags.0.get())[cpu_id() as usize];
            let head = mags.heads[cls];
            if !head.is_null() {
                // Cache hit: read intrusive next pointer, unlink head.
                let next = *(head as *const *mut u8);
                mags.heads[cls] = next;
                mags.depth[cls] -= 1;
                if saved { x86_64::instructions::interrupts::enable(); }
                return head;
            }
            if saved { x86_64::instructions::interrupts::enable(); }
            // Cache miss: allocate from talc using the CANONICAL class layout
            // so that every magazine block has the same known size. The caller
            // gets a block of class_size >= request.size with align 16 >=
            // request.align — valid per the GlobalAlloc contract.
            return self.inner.alloc(cls_layout);
        }
        self.inner.alloc(layout)
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        if let Some((cls, cls_layout)) = size_class(layout) {
            let saved = x86_64::instructions::interrupts::are_enabled();
            x86_64::instructions::interrupts::disable();
            let mags = &mut (*self.mags.0.get())[cpu_id() as usize];
            if (mags.depth[cls] as usize) < CACHE_DEPTH {
                // Push onto free-list: write old head into first word of block.
                *(p as *mut *mut u8) = mags.heads[cls];
                mags.heads[cls] = p;
                mags.depth[cls] += 1;
                if saved { x86_64::instructions::interrupts::enable(); }
                return;
            }
            if saved { x86_64::instructions::interrupts::enable(); }
            // Cache full: return to talc with the canonical class layout
            // (which is what talc allocated it with on the alloc side).
            self.inner.dealloc(p, cls_layout);
            return;
        }
        self.inner.dealloc(p, layout);
    }
}
