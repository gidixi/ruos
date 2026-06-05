//! HID Keyboard/Keypad usage IDs (page 0x07) → terminal bytes. Boot protocol.

/// Map a usage id + shift + ctrl to an output byte. Returns None for keys with
/// no terminal byte (modifiers, F-keys, etc.). Arrow keys not handled here (MVP).
pub fn usage_to_byte(usage: u8, shift: bool, ctrl: bool) -> Option<u8> {
    let base: u8 = match usage {
        0x04..=0x1D => b'a' + (usage - 0x04),   // a..z
        0x1E..=0x26 => b'1' + (usage - 0x1E),   // 1..9
        0x27 => b'0',
        0x28 => b'\n',  // Enter
        0x29 => 0x1B,   // Esc
        0x2A => 0x7F,   // Backspace -> DEL
        0x2B => b'\t',  // Tab
        0x2C => b' ',   // Space
        0x2D => b'-', 0x2E => b'=', 0x2F => b'[', 0x30 => b']',
        0x31 => b'\\', 0x33 => b';', 0x34 => b'\'', 0x35 => b'`',
        0x36 => b',', 0x37 => b'.', 0x38 => b'/',
        _ => return None,
    };
    if ctrl {
        if (b'a'..=b'z').contains(&base) { return Some(base - b'a' + 1); } // Ctrl-A..Z = 1..26
        return Some(base);
    }
    if shift { return Some(shift_byte(base)); }
    Some(base)
}

/// Map a HID keyboard usage id (page 0x07) to a PS/2 Set 1 scancode — the same
/// encoding the framebuffer GUI consumes via `gfx::push_key` (the PS/2 driver
/// sends these directly). Extended keys (arrows, Home/End/Del) use the GUI's
/// `0xE0xx` convention. Returns None for usages with no GUI scancode.
pub fn usage_to_scancode(usage: u8) -> Option<u32> {
    // a..z (usage 0x04..=0x1D) → Set 1 letter scancodes (not contiguous).
    const LETTERS: [u32; 26] = [
        0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, // a..j
        0x25, 0x26, 0x32, 0x31, 0x18, 0x19, 0x10, 0x13, 0x1F, 0x14, // k..t
        0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2C,                         // u..z
    ];
    Some(match usage {
        0x04..=0x1D => LETTERS[(usage - 0x04) as usize],
        0x1E..=0x26 => 0x02 + (usage - 0x1E) as u32, // 1..9 → K1..K9
        0x27 => 0x0B,                                 // 0
        0x28 => 0x1C,                                 // Enter
        0x29 => 0x01,                                 // Esc
        0x2A => 0x0E,                                 // Backspace
        0x2B => 0x0F,                                 // Tab
        0x2C => 0x39,                                 // Space
        0x2D => 0x0C, 0x2E => 0x0D, 0x2F => 0x1A, 0x30 => 0x1B, // - = [ ]
        0x31 => 0x2B, 0x33 => 0x27, 0x34 => 0x28, 0x35 => 0x29, // \ ; ' `
        0x36 => 0x33, 0x37 => 0x34, 0x38 => 0x35,               // , . /
        0x3A..=0x43 => 0x3B + (usage - 0x3A) as u32,  // F1..F10
        0x4A => 0xE047,                               // Home
        0x4C => 0xE053,                               // Delete
        0x4D => 0xE04F,                               // End
        0x4F => 0xE04D,                               // Right
        0x50 => 0xE04B,                               // Left
        0x51 => 0xE050,                               // Down
        0x52 => 0xE048,                               // Up
        _ => return None,
    })
}

/// Map a HID modifier-byte bit index (0=LCtrl,1=LShift,2=LAlt,3=LGui,4=RCtrl,
/// 5=RShift,6=RAlt,7=RGui) to the Set 1 scancode the GUI tracks for modifiers.
/// Returns None for the GUI keys (no modifier role).
pub fn modifier_scancode(bit: u8) -> Option<u32> {
    Some(match bit {
        0 | 4 => 0x1D, // L/R Ctrl
        1 => 0x2A,     // L Shift
        5 => 0x36,     // R Shift
        2 | 6 => 0x38, // L/R Alt
        _ => return None,
    })
}

fn shift_byte(b: u8) -> u8 {
    match b {
        b'a'..=b'z' => b - 32,
        b'1' => b'!', b'2' => b'@', b'3' => b'#', b'4' => b'$', b'5' => b'%',
        b'6' => b'^', b'7' => b'&', b'8' => b'*', b'9' => b'(', b'0' => b')',
        b'-' => b'_', b'=' => b'+', b'[' => b'{', b']' => b'}', b'\\' => b'|',
        b';' => b':', b'\'' => b'"', b'`' => b'~', b',' => b'<', b'.' => b'>',
        b'/' => b'?', other => other,
    }
}

/// Boot-check self-test: a few independently-known PS/2 Set 1 scancodes (matching
/// the GUI's `abi::scancode` constants) plus a modifier and an extended key.
#[cfg(feature = "boot-checks")]
pub fn scancode_self_test() -> bool {
    usage_to_scancode(0x04) == Some(0x1E)   // 'a'
        && usage_to_scancode(0x1D) == Some(0x2C) // 'z'
        && usage_to_scancode(0x1E) == Some(0x02) // '1'
        && usage_to_scancode(0x27) == Some(0x0B) // '0'
        && usage_to_scancode(0x28) == Some(0x1C) // Enter
        && usage_to_scancode(0x2C) == Some(0x39) // Space
        && usage_to_scancode(0x52) == Some(0xE048) // Up arrow
        && usage_to_scancode(0x3A) == Some(0x3B) // F1
        && usage_to_scancode(0xE0) == None       // raw modifier usage has no key scancode
        && modifier_scancode(1) == Some(0x2A)    // L Shift
        && modifier_scancode(0) == Some(0x1D)    // L Ctrl
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    #[test] fn letters() {
        assert_eq!(usage_to_byte(0x04, false, false), Some(b'a'));
        assert_eq!(usage_to_byte(0x04, true,  false), Some(b'A'));
        assert_eq!(usage_to_byte(0x1D, false, false), Some(b'z'));
    }
    #[test] fn digits_symbols() {
        assert_eq!(usage_to_byte(0x1E, false, false), Some(b'1'));
        assert_eq!(usage_to_byte(0x1E, true,  false), Some(b'!'));
        assert_eq!(usage_to_byte(0x27, false, false), Some(b'0'));
    }
    #[test] fn control_keys() {
        assert_eq!(usage_to_byte(0x28, false, false), Some(b'\n'));
        assert_eq!(usage_to_byte(0x2A, false, false), Some(0x7F));
        assert_eq!(usage_to_byte(0x06, false, true),  Some(0x03)); // Ctrl-C
    }
    #[test] fn no_byte() {
        assert_eq!(usage_to_byte(0xE0, false, false), None); // L-Ctrl
        assert_eq!(usage_to_byte(0x3A, false, false), None); // F1
    }
}
