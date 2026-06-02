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
