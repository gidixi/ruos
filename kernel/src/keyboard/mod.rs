//! Minimal PS/2 keyboard: read raw scancodes from port 0x60 and push
//! decoded ASCII bytes into the async queue.
//! IRQ1 is wired to `VEC_KEYBOARD` via IOAPIC redirection.

pub mod queue;

use x86_64::instructions::port::Port;
use x86_64::structures::idt::InterruptStackFrame;
use crate::{apic, idt};
use crate::acpi_init::IrqOverride;

/// Scancode Set 1 make-codes → ASCII (index = scancode byte).
/// Only printable ASCII keys are mapped; 0 means "no character".
/// This covers the standard US QWERTY layout for the lower 89 keys.
static SCANCODE_MAP: [u8; 89] = [
    // 0x00
    0,
    // 0x01 Esc
    0x1B,
    // 0x02 – 0x0B: 1 2 3 4 5 6 7 8 9 0
    b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0',
    // 0x0C 0x0D: - =
    b'-', b'=',
    // 0x0E Backspace
    0x08,
    // 0x0F Tab
    b'\t',
    // 0x10 – 0x19: q w e r t y u i o p
    b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i', b'o', b'p',
    // 0x1A 0x1B: [ ]
    b'[', b']',
    // 0x1C Enter
    b'\n',
    // 0x1D Left Ctrl
    0,
    // 0x1E – 0x26: a s d f g h j k l
    b'a', b's', b'd', b'f', b'g', b'h', b'j', b'k', b'l',
    // 0x27 0x28: ; '
    b';', b'\'',
    // 0x29 Backtick
    b'`',
    // 0x2A Left Shift
    0,
    // 0x2B Backslash
    b'\\',
    // 0x2C – 0x32: z x c v b n m
    b'z', b'x', b'c', b'v', b'b', b'n', b'm',
    // 0x33 0x34 0x35: , . /
    b',', b'.', b'/',
    // 0x36 Right Shift
    0,
    // 0x37 * (keypad)
    b'*',
    // 0x38 Left Alt
    0,
    // 0x39 Space
    b' ',
    // 0x3A – 0x44: Caps Lock, F1-F10
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    // 0x45 Num Lock, 0x46 Scroll Lock
    0, 0,
    // 0x47 – 0x53: keypad 7 8 9 - 4 5 6 + 1 2 3 0 .
    b'7', b'8', b'9', b'-', b'4', b'5', b'6', b'+', b'1', b'2', b'3', b'0', b'.',
    // 0x54 – 0x58: (unused / F11 F12 area)
    0, 0, 0, 0, 0,
];

pub extern "x86-interrupt" fn keyboard_handler(_frame: InterruptStackFrame) {
    let mut data: Port<u8> = Port::new(0x60);
    // SAFETY: 0x60 is the PS/2 controller data port.
    let scancode = unsafe { data.read() };

    // Ignore key-release events (bit 7 set) and extended prefix (0xE0).
    if scancode < 0x80 {
        let idx = scancode as usize;
        if idx < SCANCODE_MAP.len() {
            let ch = SCANCODE_MAP[idx];
            if ch != 0 {
                // Push the byte into the async queue; the consumer task
                // (kbd_echo_task) handles logging. Non-ASCII chars are
                // clamped to '?' for the queue — Step 11 will expand
                // once the shell becomes the real consumer.
                let b = if ch < 0x80 { ch } else { b'?' };
                queue::push_from_isr(b);
            }
        }
    }

    apic::lapic::eoi();
}

pub fn init(overrides: &[IrqOverride]) {
    apic::ioapic::redirect(1, idt::VEC_KEYBOARD, overrides);
}
