#![no_std]

//! Self-closing reactor guest. Draws a cycling colour like `wt-reactor`, but on
//! its 3rd `frame()` it calls `wm.close()` to request its own teardown —
//! exercises the compositor despawn path (guest close request -> drop
//! Store/Instance). No allocator: the surface is a single static array.

#[link(wasm_import_module = "wm")]
extern "C" {
    fn commit(ptr: *const u8, len: u32, w: u32, h: u32);
    fn app_id() -> u32;
    fn tick();
    fn close();
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
        // Green-ish cycling fill so it's visually distinct from the blue reactor.
        let g = (COUNTER.wrapping_add(id.wrapping_mul(50)) & 0xff) as u8;
        let p = core::ptr::addr_of_mut!(BUF) as *mut u8;
        let mut i = 0;
        while i < W * H * 4 {
            *p.add(i) = 0x20;
            *p.add(i + 1) = g;
            *p.add(i + 2) = 0x20;
            *p.add(i + 3) = 0xff;
            i += 4;
        }
        commit(core::ptr::addr_of!(BUF) as *const u8, (W * H * 4) as u32, W as u32, H as u32);
        if COUNTER == 3 {
            close();
        }
    }
}

#[panic_handler]
fn ph(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
