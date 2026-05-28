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
