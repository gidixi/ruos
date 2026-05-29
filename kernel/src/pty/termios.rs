//! POSIX termios subset for PTY line discipline.
//!
//! 56-byte `#[repr(C)]` ABI. wasi-libc's `__wasi_termios_t` and shell.wasm's
//! mirror extern "C" struct must agree on this layout exactly, since
//! tcgetattr/tcsetattr `memcpy` the struct across the wasm-kernel boundary.
//! Static-assert size below catches accidental field additions.

pub const NCCS: usize = 32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Termios {
    pub c_iflag:  u32,
    pub c_oflag:  u32,
    pub c_cflag:  u32,
    pub c_lflag:  u32,
    pub c_cc:     [u8; NCCS],
    pub c_ispeed: u32,
    pub c_ospeed: u32,
}

// Step 12 F2: ABI lock. 4×u32 + 32×u8 + 2×u32 = 56 bytes (no padding —
// all field alignments ≤ 4 bytes).
const _: () = assert!(core::mem::size_of::<Termios>() == 56);
const _: () = assert!(core::mem::align_of::<Termios>() == 4);

// c_iflag bits
pub const ICRNL:  u32 = 0o0100;

// c_oflag bits
pub const OPOST:  u32 = 0o0001;
pub const ONLCR:  u32 = 0o0004;

// c_lflag bits
pub const ISIG:   u32 = 0o0001;
pub const ICANON: u32 = 0o0002;
pub const ECHO:   u32 = 0o0010;
pub const IEXTEN: u32 = 0o100000;

// c_cc indices
pub const VINTR:  usize = 0;
pub const VERASE: usize = 2;
pub const VEOF:   usize = 4;
pub const VEOL:   usize = 5;

impl Termios {
    pub const fn default_cooked() -> Self {
        let mut cc = [0u8; NCCS];
        cc[VINTR]  = 0x03;
        cc[VERASE] = 0x7F;
        cc[VEOF]   = 0x04;
        cc[VEOL]   = 0x00;
        Self {
            c_iflag:  ICRNL,
            c_oflag:  OPOST | ONLCR,
            c_cflag:  0,
            c_lflag:  ISIG | ICANON | ECHO | IEXTEN,
            c_cc:     cc,
            c_ispeed: 38400,
            c_ospeed: 38400,
        }
    }
}
