//! PS/2 keyboard ISR. Scancode → ASCII (regular) or ANSI escape
//! sequence (extended), pushed into PTY 0 master input.
//!
//! 0xE0 extended scancodes are latched via an AtomicBool; the following
//! non-release scancode is translated to an ANSI escape sequence.
//! Regular scancodes are decoded via SCANCODE_MAP (Set 1 → ASCII).

use core::sync::atomic::{AtomicBool, Ordering};
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

/// Latch for the 0xE0 extended scancode prefix.
static EXTENDED: AtomicBool = AtomicBool::new(false);

/// Map an extended (post-0xE0) scancode to its ANSI escape sequence.
/// Returns None for scancodes without a defined mapping.
fn extended_to_ansi(scancode: u8) -> Option<&'static [u8]> {
    match scancode {
        0x48 => Some(b"\x1b[A"),   // Up arrow
        0x50 => Some(b"\x1b[B"),   // Down arrow
        0x4D => Some(b"\x1b[C"),   // Right arrow
        0x4B => Some(b"\x1b[D"),   // Left arrow
        0x47 => Some(b"\x1b[H"),   // Home
        0x4F => Some(b"\x1b[F"),   // End
        0x53 => Some(b"\x1b[3~"),  // Delete
        _ => None,
    }
}

pub extern "x86-interrupt" fn keyboard_handler(_frame: InterruptStackFrame) {
    let mut data: Port<u8> = Port::new(0x60);
    // SAFETY: 0x60 is the PS/2 controller data port.
    let scancode = unsafe { data.read() };

    // 0xE0 prefix: latch the EXTENDED flag and discard this byte.
    if scancode == 0xE0 {
        EXTENDED.store(true, Ordering::SeqCst);
        apic::lapic::eoi();
        return;
    }

    // If the previous byte was 0xE0, decode as extended scancode.
    if EXTENDED.swap(false, Ordering::SeqCst) {
        // Only process make-codes (bit 7 clear = press, not release).
        if scancode < 0x80 {
            if let Some(seq) = extended_to_ansi(scancode) {
                for &b in seq {
                    crate::pty::master_input_push(0, b);
                }
            }
        }
        apic::lapic::eoi();
        return;
    }

    // Regular scancode: ignore key-release events (bit 7 set).
    if scancode < 0x80 {
        let idx = scancode as usize;
        if idx < SCANCODE_MAP.len() {
            let ch = SCANCODE_MAP[idx];
            if ch != 0 {
                crate::pty::master_input_push(0, ch);
            }
        }
    }

    apic::lapic::eoi();
}

pub fn init(overrides: &[IrqOverride]) {
    apic::ioapic::redirect(1, idt::VEC_KEYBOARD, overrides);
}
