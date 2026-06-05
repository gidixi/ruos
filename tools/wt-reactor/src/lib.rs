#![no_std]

//! Minimal no_std reactor guest for the compositor GATE spike. Exports `frame()`
//! and imports the raw `wm` host module (`commit`, `app_id`, `tick`). Each
//! `frame()` ticks the host, bumps a static counter, fills a static RGBA buffer
//! with a colour that cycles per frame (offset by this instance's `app_id`), and
//! commits the surface. No allocator: the buffer is a single static array.

#[link(wasm_import_module = "wm")]
extern "C" {
    fn commit(ptr: *const u8, len: u32, w: u32, h: u32);
    fn app_id() -> u32;
    fn tick();
}

const W: usize = 320;
const H: usize = 240;
static mut BUF: [u8; W * H * 4] = [0; W * H * 4];
static mut COUNTER: u32 = 0;

#[no_mangle]
pub extern "C" fn frame() {
    unsafe {
        tick();
        COUNTER = COUNTER.wrapping_add(1);
        let id = app_id();
        let r = (COUNTER.wrapping_add(id.wrapping_mul(80)) & 0xff) as u8;
        let p = core::ptr::addr_of_mut!(BUF) as *mut u8;
        let mut i = 0;
        while i < W * H * 4 {
            *p.add(i) = r;
            *p.add(i + 1) = 0x40;
            *p.add(i + 2) = 0x80;
            *p.add(i + 3) = 0xff;
            i += 4;
        }
        commit(core::ptr::addr_of!(BUF) as *const u8, (W * H * 4) as u32, W as u32, H as u32);
    }
}

#[panic_handler]
fn ph(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
