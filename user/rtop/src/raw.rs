//! Raw-mode + alt-screen RAII guard using ruos tcgetattr/tcsetattr host fns.
//! Restores the terminal on drop/q/panic — never leave the PTY in raw mode.
//!
//! Termios layout (from kernel/src/pty/termios.rs, 56 bytes total, repr(C)):
//!   offset  0: c_iflag  (u32)
//!   offset  4: c_oflag  (u32)
//!   offset  8: c_cflag  (u32)
//!   offset 12: c_lflag  (u32)  ← clear ICANON|ECHO|ISIG here
//!   offset 16: c_cc     ([u8; 32])
//!   offset 48: c_ispeed (u32)
//!   offset 52: c_ospeed (u32)
//!   total = 56 bytes
use std::io::Write;

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn tcgetattr(fd: i32, termios_ptr: u32) -> i32;
    fn tcsetattr(fd: i32, optional_actions: i32, termios_ptr: u32) -> i32;
}

// Verified against kernel/src/pty/termios.rs (4×u32 + 32×u8 + 2×u32 = 56).
const TERMIOS_LEN: usize = 56;
// c_lflag is the 4th u32: offset = 3 * 4 = 12.
const LFLAG_OFF: usize = 12;
// c_lflag bits (octal, matching termios.rs):
const ISIG:   u32 = 0o0001; // 0x0001
const ICANON: u32 = 0o0002; // 0x0002
const ECHO:   u32 = 0o0010; // 0x0008
// tcsetattr action — kernel ignores it but we pass TCSANOW = 0 (matches nano).
const TCSANOW: i32 = 0;

pub struct TermGuard {
    saved: [u8; TERMIOS_LEN],
}

impl TermGuard {
    pub fn enter() -> Self {
        let mut t = [0u8; TERMIOS_LEN];
        unsafe { tcgetattr(0, t.as_mut_ptr() as u32); }
        let saved = t;
        // Read, clear ICANON | ECHO | ISIG, write back.
        let mut lflag = u32::from_le_bytes([
            t[LFLAG_OFF], t[LFLAG_OFF + 1], t[LFLAG_OFF + 2], t[LFLAG_OFF + 3],
        ]);
        lflag &= !(ICANON | ECHO | ISIG);
        t[LFLAG_OFF..LFLAG_OFF + 4].copy_from_slice(&lflag.to_le_bytes());
        unsafe { tcsetattr(0, TCSANOW, t.as_mut_ptr() as u32); }
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x1b[?1049h\x1b[?25l\x1b[2J");
        let _ = out.flush();
        TermGuard { saved }
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x1b[?25h\x1b[?1049l");
        let _ = out.flush();
        unsafe { tcsetattr(0, TCSANOW, self.saved.as_mut_ptr() as u32); }
    }
}
