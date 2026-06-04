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

use bitflags::bitflags;

bitflags! {
    /// Attributi testo per cella. Bold/dim/underline/reverse sono definiti ora
    /// per stabilizzare il tipo; il rendering degli attributi arriva nel Plan 2.
    #[derive(Debug, Copy, Clone, PartialEq, Eq)]
    pub struct CellAttr: u8 {
        const BOLD      = 0b0001;
        const DIM       = 0b0010;
        const UNDERLINE = 0b0100;
        const REVERSE   = 0b1000;
    }
}

/// Una cella della griglia terminale.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Cell {
    pub ch:   char,
    pub fg:   Rgb,
    pub bg:   Rgb,
    pub attr: CellAttr,
}

impl Cell {
    /// Cella vuota (spazio) con i colori dati e nessun attributo.
    pub fn blank(fg: Rgb, bg: Rgb) -> Self {
        Cell { ch: ' ', fg, bg, attr: CellAttr::empty() }
    }
}

/// Apply a CSI SGR parameter sequence to fg/bg/attr. Unknown params ignored.
/// 0 reset-all; 1/2/4/7 bold/dim/underline/reverse; 22/24/27 reset those;
/// 30-37/90-97 fg, 40-47/100-107 bg (16-color); 39/49 default fg/bg;
/// 38;5;N / 48;5;N indexed; 38;2;r;g;b / 48;2;r;g;b truecolor.
pub fn apply_sgr(
    mut params: impl Iterator<Item = u16>,
    mut fg: Rgb, mut bg: Rgb, mut attr: CellAttr,
) -> (Rgb, Rgb, CellAttr) {
    while let Some(p) = params.next() {
        match p {
            0 => { fg = WHITE; bg = BLACK; attr = CellAttr::empty(); }
            1 => attr.insert(CellAttr::BOLD),
            2 => attr.insert(CellAttr::DIM),
            4 => attr.insert(CellAttr::UNDERLINE),
            7 => attr.insert(CellAttr::REVERSE),
            22 => attr.remove(CellAttr::BOLD | CellAttr::DIM),
            24 => attr.remove(CellAttr::UNDERLINE),
            27 => attr.remove(CellAttr::REVERSE),
            30..=37   => fg = VGA_16[(p - 30) as usize],
            39        => fg = WHITE,
            40..=47   => bg = VGA_16[(p - 40) as usize],
            49        => bg = BLACK,
            90..=97   => fg = VGA_16[((p - 90) + 8) as usize],
            100..=107 => bg = VGA_16[((p - 100) + 8) as usize],
            38 => match params.next() {
                Some(5) => { if let Some(i) = params.next() { fg = xterm_256(i as u8); } }
                Some(2) => {
                    let r = params.next().unwrap_or(0) as u8;
                    let g = params.next().unwrap_or(0) as u8;
                    let b = params.next().unwrap_or(0) as u8;
                    fg = Rgb { r, g, b };
                }
                _ => {}
            },
            48 => match params.next() {
                Some(5) => { if let Some(i) = params.next() { bg = xterm_256(i as u8); } }
                Some(2) => {
                    let r = params.next().unwrap_or(0) as u8;
                    let g = params.next().unwrap_or(0) as u8;
                    let b = params.next().unwrap_or(0) as u8;
                    bg = Rgb { r, g, b };
                }
                _ => {}
            },
            _ => {}
        }
    }
    (fg, bg, attr)
}
