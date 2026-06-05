#![no_std]

//! Minimal no_std reactor guest for the compositor GATE spike. Exports `frame()`
//! and imports the raw `wm` host module (`commit`, `app_id`, `tick`,
//! `poll_event`). Each `frame()` drains its window's input queue via
//! `wm.poll_event` (counting mouse-button-down clicks), ticks the host, fills a
//! static RGBA buffer with a colour chosen by `CLICKS % 5` (offset by this
//! instance's `app_id` so the two windows differ at click 0), and commits the
//! surface. No allocator: the buffer is a single static array.

#[link(wasm_import_module = "wm")]
extern "C" {
    fn commit(ptr: *const u8, len: u32, w: u32, h: u32);
    fn app_id() -> u32;
    fn tick();
    fn poll_event(ptr: *mut u8);
}

const W: usize = 320;
const H: usize = 240;
static mut BUF: [u8; W * H * 4] = [0; W * H * 4];
static mut COUNTER: u32 = 0;
static mut CLICKS: u32 = 0;

/// Per-click palette (RGBA bytes, written R,G,B,A to match the existing fill).
/// Five visibly-distinct colours; `CLICKS % 5` indexes this.
const PALETTE: [[u8; 4]; 5] = [
    [0x20, 0x20, 0x60, 0xff], // dark blue
    [0x20, 0x70, 0x20, 0xff], // green
    [0x80, 0x20, 0x20, 0xff], // red
    [0x60, 0x60, 0x20, 0xff], // olive
    [0x20, 0x60, 0x80, 0xff], // teal
];

#[no_mangle]
pub extern "C" fn frame() {
    unsafe {
        // Drain ALL queued input events for THIS window before drawing; count
        // mouse-button-DOWN events as clicks (kind==2, p1!=0 = pressed).
        let mut ev = [0u8; 20];
        loop {
            poll_event(ev.as_mut_ptr());
            let disc = u32::from_le_bytes([ev[0], ev[1], ev[2], ev[3]]);
            if disc == 0 {
                break;
            }
            let kind = u32::from_le_bytes([ev[4], ev[5], ev[6], ev[7]]);
            let p1 = u32::from_le_bytes([ev[12], ev[13], ev[14], ev[15]]);
            if kind == 2 && p1 != 0 {
                CLICKS = CLICKS.wrapping_add(1);
            }
        }

        tick();
        COUNTER = COUNTER.wrapping_add(1);
        let id = app_id();
        // Base colour depends on CLICKS so a click is visible; offset the index
        // by app_id so the two windows look different at click 0.
        let idx = (CLICKS.wrapping_add(id)) % 5;
        let c = PALETTE[idx as usize];
        let p = core::ptr::addr_of_mut!(BUF) as *mut u8;
        let mut i = 0;
        while i < W * H * 4 {
            *p.add(i) = c[0];
            *p.add(i + 1) = c[1];
            *p.add(i + 2) = c[2];
            *p.add(i + 3) = c[3];
            i += 4;
        }
        commit(core::ptr::addr_of!(BUF) as *const u8, (W * H * 4) as u32, W as u32, H as u32);
    }
}

#[panic_handler]
fn ph(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
