//! wasip1 STD reactor probe (SP-A). Proves a wasm32-wasip1 std guest runs as a
//! compositor window: it allocates on the heap (std), fills its surface, and
//! commits via `wm`. No egui — this only exercises WASI + wm on one linker.
//!
//! A `cdylib` with only a `#[no_mangle] frame` export (no `main`/`_start`)
//! builds as a wasip1 *reactor*: wasm-ld emits an `_initialize` export (runs
//! std's static initializers) plus the `wm` + `wasi_snapshot_preview1` imports
//! std references. The compositor calls `_initialize` once before `frame()`.

#[link(wasm_import_module = "wm")]
extern "C" {
    fn commit(ptr: *const u8, len: u32, w: u32, h: u32);
    fn app_id() -> u32;
    fn tick();
}

const W: usize = 320;
const H: usize = 240;

#[no_mangle]
pub extern "C" fn frame() {
    unsafe {
        tick();
    }
    let id = unsafe { app_id() };
    // STD heap allocation (the whole point: proves the std/libc/WASI runtime works).
    let mut buf: Vec<u8> = Vec::with_capacity(W * H * 4);
    // A solid colour that depends on app_id, like the no_std reactor, so the
    // window is visibly distinct.
    let (r, g, b) = (
        0x30u8,
        0x80u8.wrapping_add((id as u8).wrapping_mul(40)),
        0xB0u8,
    );
    for _ in 0..(W * H) {
        buf.push(r);
        buf.push(g);
        buf.push(b);
        buf.push(0xFF);
    }
    unsafe {
        commit(buf.as_ptr(), (W * H * 4) as u32, W as u32, H as u32);
    }
}
