#![no_std]

//! Deliberately-misbehaving reactor guest for the epoch-watchdog boot-check.
//! Commits one frame (so the spawn looks healthy), then on its 2nd `frame()`
//! enters an infinite busy-loop — without the watchdog this would freeze the
//! compositor forever; with it the call traps (`Trap::Interrupt`) and the
//! window is killed. No allocator: the surface is a single static array.

#[link(wasm_import_module = "wm")]
extern "C" {
    fn commit(ptr: *const u8, len: u32, w: u32, h: u32);
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
        if COUNTER >= 2 {
            // Runaway: spin forever. The loop body keeps a volatile read so the
            // optimizer cannot elide the loop; cranelift emits an epoch check on
            // the backedge, which is exactly what the watchdog needs.
            let mut x: u32 = 0;
            loop {
                x = x.wrapping_add(core::ptr::read_volatile(&COUNTER));
                core::ptr::write_volatile(core::ptr::addr_of_mut!(COUNTER), x | 1);
            }
        }
        // Frame 1: solid red fill, one commit — proves the spawn was healthy.
        let p = core::ptr::addr_of_mut!(BUF) as *mut u8;
        let mut i = 0;
        while i < W * H * 4 {
            *p.add(i) = 0xe0;
            *p.add(i + 1) = 0x20;
            *p.add(i + 2) = 0x20;
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
