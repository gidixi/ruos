//! A ratatui Backend that emits ANSI escape sequences to stdout via
//! `std::io::Write`. We bypass crossterm/termion (they need termios ioctls
//! WASI lacks); raw mode is handled separately via ruos tcsetattr in main.

use std::io::{self, Write};
use ratatui::backend::{Backend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size, Rect};
use ratatui::style::{Color, Modifier};

pub struct AnsiBackend {
    out: io::Stdout,
    width: u16,
    height: u16,
}

impl AnsiBackend {
    pub fn new(width: u16, height: u16) -> Self {
        Self { out: io::stdout(), width, height }
    }
}

fn ansi_index(c: Color) -> Option<u8> {
    Some(match c {
        Color::Black => 0, Color::Red => 1, Color::Green => 2, Color::Yellow => 3,
        Color::Blue => 4, Color::Magenta => 5, Color::Cyan => 6, Color::Gray => 7,
        _ => return None,
    })
}

fn sgr_for(cell: &Cell, s: &mut String) {
    use core::fmt::Write as _;
    s.push_str("\x1b[0m");
    if let Color::Rgb(r, g, b) = cell.fg {
        let _ = write!(s, "\x1b[38;2;{};{};{}m", r, g, b);
    } else if let Some(n) = ansi_index(cell.fg) {
        let _ = write!(s, "\x1b[{}m", 30 + n);
    }
    if let Color::Rgb(r, g, b) = cell.bg {
        let _ = write!(s, "\x1b[48;2;{};{};{}m", r, g, b);
    } else if let Some(n) = ansi_index(cell.bg) {
        let _ = write!(s, "\x1b[{}m", 40 + n);
    }
    if cell.modifier.contains(Modifier::BOLD) { s.push_str("\x1b[1m"); }
    if cell.modifier.contains(Modifier::REVERSED) { s.push_str("\x1b[7m"); }
}

impl Backend for AnsiBackend {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where I: Iterator<Item = (u16, u16, &'a Cell)> {
        let mut s = String::new();
        for (x, y, cell) in content {
            s.push_str(&format!("\x1b[{};{}H", y + 1, x + 1));
            sgr_for(cell, &mut s);
            s.push_str(cell.symbol());
        }
        s.push_str("\x1b[0m");
        self.out.write_all(s.as_bytes())
    }

    fn hide_cursor(&mut self) -> io::Result<()> { self.out.write_all(b"\x1b[?25l") }
    fn show_cursor(&mut self) -> io::Result<()> { self.out.write_all(b"\x1b[?25h") }

    fn get_cursor_position(&mut self) -> io::Result<Position> { Ok(Position::new(0, 0)) }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let p = position.into();
        self.out.write_all(format!("\x1b[{};{}H", p.y + 1, p.x + 1).as_bytes())
    }

    fn clear(&mut self) -> io::Result<()> { self.out.write_all(b"\x1b[2J") }

    fn size(&self) -> io::Result<Size> { Ok(Size::new(self.width, self.height)) }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        Ok(WindowSize {
            columns_rows: Size::new(self.width, self.height),
            pixels: Size::new(0, 0),
        })
    }

    fn flush(&mut self) -> io::Result<()> { self.out.flush() }
}

/// Helper kept for symmetry with potential clear_region usage.
#[allow(dead_code)]
pub fn full_area(w: u16, h: u16) -> Rect { Rect::new(0, 0, w, h) }
