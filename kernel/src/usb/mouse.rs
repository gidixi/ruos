//! USB HID boot mouse: report decode.
//!
//! The boot-protocol mouse report is 3+ bytes: byte0 = buttons (bit0 L, bit1 R,
//! bit2 M), byte1 = X delta (i8), byte2 = Y delta (i8). Unlike PS/2 (which
//! reports up as positive), USB HID reports +Y as DOWN — already the convention
//! of `crate::mouse::MouseEvent` — so no Y negation. Any wheel byte is ignored.

use crate::mouse::MouseEvent;

/// Decode a USB HID boot-protocol mouse report into a `MouseEvent`. Bytes beyond
/// the first three (e.g. a wheel byte) are ignored; a short report decodes the
/// missing fields as zero.
pub fn decode_boot_mouse(report: &[u8]) -> MouseEvent {
    let buttons = report.first().copied().unwrap_or(0);
    // Bytes 1/2 are i8 deltas; a short report leaves the missing axis at 0.
    let dx = report.get(1).map_or(0, |&b| b as i8 as i16);
    let dy = report.get(2).map_or(0, |&b| b as i8 as i16);
    MouseEvent {
        dx,
        dy, // USB HID: +Y is down → already the MouseEvent convention, no flip.
        left:   buttons & 0x01 != 0,
        right:  buttons & 0x02 != 0,
        middle: buttons & 0x04 != 0,
    }
}

/// Boot-check self-test: deterministic reports exercise button bits, i8
/// sign-extension on both axes, and the no-Y-negation convention.
#[cfg(feature = "boot-checks")]
pub fn self_test() -> bool {
    // No buttons, no movement.
    if decode_boot_mouse(&[0x00, 0x00, 0x00])
        != (MouseEvent { dx: 0, dy: 0, left: false, right: false, middle: false })
    {
        return false;
    }
    // Left button, +5 right, +3 down (USB +Y is down → not negated).
    if decode_boot_mouse(&[0x01, 0x05, 0x03])
        != (MouseEvent { dx: 5, dy: 3, left: true, right: false, middle: false })
    {
        return false;
    }
    // Right+middle, -2 (0xFE), -1 (0xFF) — i8 sign-extension on both axes.
    if decode_boot_mouse(&[0x06, 0xFE, 0xFF])
        != (MouseEvent { dx: -2, dy: -1, left: false, right: true, middle: true })
    {
        return false;
    }
    // Short report: missing Y decodes as 0, not a panic.
    if decode_boot_mouse(&[0x01, 0x04])
        != (MouseEvent { dx: 4, dy: 0, left: true, right: false, middle: false })
    {
        return false;
    }
    true
}
