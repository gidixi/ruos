//! mtwin — gate Fase 2.5: finestra threaded. Al primo `frame()` spawna un
//! worker `std::thread` che incrementa un contatore atomico fino a 1000, poi
//! dorme 10 ms (poll_oneoff su fiber) e chiude a 1001. Ogni `frame()` scrive
//! il contatore nei primi 4 byte della surface committata: il gate kernel
//! legge i pixel e verifica che il worker sia avanzato TRA i frame.
//!
//! I byte 4..8 della surface sono flag di stadio (diagnostica del gate):
//! [4]=entry raggiunta, [5]=malloc ok, [6]=spawn fatto.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

#[link(wasm_import_module = "wm")]
extern "C" {
    fn commit(ptr: *const u8, len: u32, w: u32, h: u32);
    fn tick();
}

static COUNTER: AtomicU32 = AtomicU32::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);

const W: usize = 64;
const H: usize = 64;
static mut BUF: [u8; W * H * 4] = [0; W * H * 4];

#[no_mangle]
pub extern "C" fn frame() {
    // Stage A: entry + commit statico (niente malloc, niente thread).
    unsafe {
        BUF[4] = 1;
        tick();
        commit(core::ptr::addr_of!(BUF) as *const u8, (W * H * 4) as u32, W as u32, H as u32);
    }
    // Stage B: primo malloc (dlmalloc sulla shared memory).
    let probe = alloc_probe();
    unsafe { BUF[5] = probe; }
    // Stage C: spawn del worker (una sola volta).
    if !STARTED.swap(true, Ordering::SeqCst) {
        std::thread::spawn(|| {
            for _ in 0..1000 {
                COUNTER.fetch_add(1, Ordering::SeqCst);
            }
            // poll_oneoff dal worker: il fiber si parcheggia, core libero.
            std::thread::sleep(std::time::Duration::from_millis(10));
            COUNTER.fetch_add(1, Ordering::SeqCst); // 1001 = done
        });
        unsafe { BUF[6] = 1; }
    }
    let c = COUNTER.load(Ordering::SeqCst);
    unsafe {
        BUF[0..4].copy_from_slice(&c.to_le_bytes());
        commit(core::ptr::addr_of!(BUF) as *const u8, (W * H * 4) as u32, W as u32, H as u32);
    }
}

#[inline(never)]
fn alloc_probe() -> u8 {
    let b = std::hint::black_box(Box::new(0xABu8));
    if *b == 0xAB { 1 } else { 2 }
}
