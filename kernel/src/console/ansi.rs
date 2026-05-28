//! Color types and the basic 16-color VGA palette. SGR parsing is added in
//! Task 4 (ANSI escapes); for Task 1 we only need WHITE/BLACK.

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

pub const WHITE: Rgb = Rgb { r: 0xEE, g: 0xEE, b: 0xEE };
pub const BLACK: Rgb = Rgb { r: 0x00, g: 0x00, b: 0x00 };

/// VGA-style 16-color palette indexed 0..15: 8 dim + 8 bright.
pub const VGA_16: [Rgb; 16] = [
    Rgb { r: 0x00, g: 0x00, b: 0x00 }, // 0 black
    Rgb { r: 0xAA, g: 0x00, b: 0x00 }, // 1 red
    Rgb { r: 0x00, g: 0xAA, b: 0x00 }, // 2 green
    Rgb { r: 0xAA, g: 0x55, b: 0x00 }, // 3 yellow (brown)
    Rgb { r: 0x00, g: 0x00, b: 0xAA }, // 4 blue
    Rgb { r: 0xAA, g: 0x00, b: 0xAA }, // 5 magenta
    Rgb { r: 0x00, g: 0xAA, b: 0xAA }, // 6 cyan
    Rgb { r: 0xAA, g: 0xAA, b: 0xAA }, // 7 white (light gray)
    Rgb { r: 0x55, g: 0x55, b: 0x55 }, // 8 bright black
    Rgb { r: 0xFF, g: 0x55, b: 0x55 }, // 9 bright red
    Rgb { r: 0x55, g: 0xFF, b: 0x55 }, // 10 bright green
    Rgb { r: 0xFF, g: 0xFF, b: 0x55 }, // 11 bright yellow
    Rgb { r: 0x55, g: 0x55, b: 0xFF }, // 12 bright blue
    Rgb { r: 0xFF, g: 0x55, b: 0xFF }, // 13 bright magenta
    Rgb { r: 0x55, g: 0xFF, b: 0xFF }, // 14 bright cyan
    Rgb { r: 0xFF, g: 0xFF, b: 0xFF }, // 15 bright white
];

/// xterm 256-color → Rgb. 0-15 = VGA_16. 16-231 = 6x6x6 RGB cube. 232-255 =
/// 24-step grayscale.
pub fn xterm_256(idx: u8) -> Rgb {
    if idx < 16 { return VGA_16[idx as usize]; }
    if idx < 232 {
        let i = idx - 16;
        let r = (i / 36) % 6;
        let g = (i / 6)  % 6;
        let b =  i       % 6;
        let to8 = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
        return Rgb { r: to8(r), g: to8(g), b: to8(b) };
    }
    let v = 8 + (idx - 232) * 10;
    Rgb { r: v, g: v, b: v }
}

/// Apply a CSI SGR parameter sequence to fg/bg. Unknown params are ignored.
/// Supports:
///   0      reset
///   30..37 fg from VGA_16[0..7]
///   40..47 bg from VGA_16[0..7]
///   90..97 fg from VGA_16[8..15]
///   100..107 bg from VGA_16[8..15]
///   38;5;N fg from xterm_256(N)
///   48;5;N bg from xterm_256(N)
pub fn apply_sgr(mut params: impl Iterator<Item = u16>, mut fg: Rgb, mut bg: Rgb) -> (Rgb, Rgb) {
    while let Some(p) = params.next() {
        match p {
            0 => { fg = WHITE; bg = BLACK; }
            30..=37   => fg = VGA_16[(p - 30) as usize],
            40..=47   => bg = VGA_16[(p - 40) as usize],
            90..=97   => fg = VGA_16[((p - 90) + 8) as usize],
            100..=107 => bg = VGA_16[((p - 100) + 8) as usize],
            38 => {
                if params.next() == Some(5) {
                    if let Some(idx) = params.next() {
                        fg = xterm_256(idx as u8);
                    }
                }
            }
            48 => {
                if params.next() == Some(5) {
                    if let Some(idx) = params.next() {
                        bg = xterm_256(idx as u8);
                    }
                }
            }
            _ => {}
        }
    }
    (fg, bg)
}
