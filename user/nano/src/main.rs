//! Minimal nano-style text editor for ruos.
//!
//! - Hardcoded 80 × 24 terminal (24 lines content, status + help footer).
//! - Raw line-discipline (ICANON/ECHO/ISIG off) via tcgetattr/tcsetattr.
//! - Buffer = Vec<String>, one entry per line, no trailing newlines.
//! - Cursor in (line, col). Scroll-on-cursor-leaves-viewport.
//! - Keys:
//!     printable ASCII -> insert at cursor
//!     Backspace (0x7F or 0x08) -> delete prev (joins lines at col 0)
//!     Enter (\r or \n)         -> split line at cursor
//!     Arrows (ESC [ A/B/C/D)   -> move cursor
//!     Home / End (ESC [ H/F)   -> col 0 / end of line
//!     ^O                       -> save
//!     ^X                       -> quit (no save prompt — caller re-runs nano)

use std::fs;
use std::io::{Read, Write};

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn tcgetattr(fd: i32, ptr: u32) -> i32;
    fn tcsetattr(fd: i32, action: i32, ptr: u32) -> i32;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Termios {
    iflag: u32,
    oflag: u32,
    cflag: u32,
    lflag: u32,
    cc: [u8; 32],
    ispeed: u32,
    ospeed: u32,
}

impl Termios {
    fn zero() -> Self {
        Self { iflag: 0, oflag: 0, cflag: 0, lflag: 0, cc: [0; 32], ispeed: 0, ospeed: 0 }
    }
}

const ICANON: u32 = 0o0002;
const ECHO:   u32 = 0o0010;
const ISIG:   u32 = 0o0001;

const COLS:   usize = 80;
const ROWS:   usize = 24;
const VIEWPORT_ROWS: usize = ROWS - 2; // 2 footer rows

fn save_and_raw() -> Termios {
    let mut saved = Termios::zero();
    unsafe { tcgetattr(0, &mut saved as *mut _ as u32); }
    let mut raw = saved;
    raw.lflag &= !(ICANON | ECHO | ISIG);
    unsafe { tcsetattr(0, 0, &raw as *const _ as u32); }
    saved
}
fn restore(t: &Termios) { unsafe { tcsetattr(0, 0, t as *const _ as u32); } }

fn read_byte() -> Option<u8> {
    let mut b = [0u8; 1];
    match std::io::stdin().read(&mut b) {
        Ok(1) => Some(b[0]),
        _ => None,
    }
}

/// Read one logical keystroke. Returns:
///   Key::Char(c)  printable byte
///   Key::Enter
///   Key::Backspace
///   Key::Up / Down / Left / Right
///   Key::Home / End
///   Key::Ctrl(c)  control byte (raw)
enum Key {
    Char(u8),
    Enter,
    Backspace,
    Up, Down, Left, Right,
    Home, End,
    Ctrl(u8),
}

fn read_key() -> Option<Key> {
    let b = read_byte()?;
    if b == 0x1B {
        // ESC sequence — read next two bytes (best effort).
        let b1 = read_byte().unwrap_or(0);
        let b2 = read_byte().unwrap_or(0);
        if b1 == b'[' {
            return Some(match b2 {
                b'A' => Key::Up,
                b'B' => Key::Down,
                b'C' => Key::Right,
                b'D' => Key::Left,
                b'H' => Key::Home,
                b'F' => Key::End,
                _    => Key::Ctrl(0x1B),
            });
        }
        return Some(Key::Ctrl(0x1B));
    }
    Some(match b {
        b'\r' | b'\n'      => Key::Enter,
        0x08 | 0x7F        => Key::Backspace,
        c if c < 0x20      => Key::Ctrl(c),
        c                  => Key::Char(c),
    })
}

struct Editor {
    lines:    Vec<String>,
    cur_line: usize,
    cur_col:  usize,
    top_line: usize, // first line visible
    filename: String,
    modified: bool,
    status:   String,
}

impl Editor {
    fn new(filename: String, content: String) -> Self {
        let mut lines: Vec<String> = if content.is_empty() {
            vec![String::new()]
        } else {
            content.split('\n').map(|s| s.to_string()).collect()
        };
        // Drop a trailing empty line caused by terminating "\n".
        if lines.len() > 1 && lines.last().map(|l| l.is_empty()).unwrap_or(false) {
            lines.pop();
        }
        Self {
            lines, cur_line: 0, cur_col: 0, top_line: 0,
            filename, modified: false, status: String::new(),
        }
    }

    fn render(&self) {
        let mut out = String::new();
        // Clear + home.
        out.push_str("\x1b[2J\x1b[H");
        for row in 0..VIEWPORT_ROWS {
            let line_idx = self.top_line + row;
            if let Some(line) = self.lines.get(line_idx) {
                // Truncate to COLS, no horizontal scroll yet.
                let display: String = line.chars().take(COLS).collect();
                out.push_str(&display);
            } else {
                out.push('~');
            }
            out.push_str("\x1b[K\r\n");
        }
        // Status bar (inverted).
        let dirty = if self.modified { "*" } else { " " };
        let pos = alloc_format!("{}/{}", self.cur_line + 1, self.lines.len());
        let bar = alloc_format!(
            " ruos nano  {}{}  Ln {}  Col {}",
            dirty, self.filename, pos, self.cur_col + 1
        );
        let pad = COLS.saturating_sub(bar.chars().count());
        out.push_str("\x1b[7m");
        out.push_str(&bar);
        for _ in 0..pad { out.push(' '); }
        out.push_str("\x1b[0m\r\n");
        // Help footer.
        out.push_str("\x1b[7m");
        let help = " ^O Save  ^X Exit  Arrows / Home / End move ";
        out.push_str(help);
        let hpad = COLS.saturating_sub(help.chars().count());
        for _ in 0..hpad { out.push(' '); }
        out.push_str("\x1b[0m");
        // Status message (overlays the help line briefly if set).
        if !self.status.is_empty() {
            out.push_str("\r");
            out.push_str("\x1b[7m");
            let trimmed: String = self.status.chars().take(COLS).collect();
            out.push_str(&trimmed);
            let spad = COLS.saturating_sub(trimmed.chars().count());
            for _ in 0..spad { out.push(' '); }
            out.push_str("\x1b[0m");
        }
        // Cursor: ANSI is 1-based, viewport row = cur_line - top_line.
        let row = self.cur_line - self.top_line + 1;
        let col = self.cur_col + 1;
        out.push_str(&alloc_format!("\x1b[{};{}H", row, col));
        std::io::stdout().write_all(out.as_bytes()).ok();
        std::io::stdout().flush().ok();
    }

    fn scroll_into_view(&mut self) {
        if self.cur_line < self.top_line {
            self.top_line = self.cur_line;
        } else if self.cur_line >= self.top_line + VIEWPORT_ROWS {
            self.top_line = self.cur_line + 1 - VIEWPORT_ROWS;
        }
    }

    fn clamp_col(&mut self) {
        let max = self.lines[self.cur_line].chars().count();
        if self.cur_col > max { self.cur_col = max; }
        if self.cur_col > COLS { self.cur_col = COLS; }
    }

    fn insert_char(&mut self, c: u8) {
        let line = &mut self.lines[self.cur_line];
        if line.is_char_boundary(self.cur_col) {
            line.insert(self.cur_col, c as char);
        } else {
            line.push(c as char);
        }
        self.cur_col += 1;
        self.modified = true;
    }

    fn enter(&mut self) {
        let rest = self.lines[self.cur_line].split_off(self.cur_col);
        self.cur_line += 1;
        self.cur_col = 0;
        self.lines.insert(self.cur_line, rest);
        self.modified = true;
    }

    fn backspace(&mut self) {
        if self.cur_col > 0 {
            let line = &mut self.lines[self.cur_line];
            let new_col = self.cur_col - 1;
            // Remove the char at new_col.
            if line.is_char_boundary(new_col) {
                line.remove(new_col);
                self.cur_col = new_col;
                self.modified = true;
            }
        } else if self.cur_line > 0 {
            // Join with previous line.
            let cur = self.lines.remove(self.cur_line);
            self.cur_line -= 1;
            self.cur_col = self.lines[self.cur_line].chars().count();
            self.lines[self.cur_line].push_str(&cur);
            self.modified = true;
        }
    }

    fn save(&mut self) {
        // Join with \n; add trailing newline if last line non-empty.
        let mut content = self.lines.join("\n");
        if !content.ends_with('\n') { content.push('\n'); }
        match fs::write(&self.filename, content.as_bytes()) {
            Ok(()) => {
                self.modified = false;
                self.status = alloc_format!(
                    " saved {} ({} lines, {} bytes) ",
                    self.filename, self.lines.len(), content.len(),
                );
            }
            Err(e) => self.status = alloc_format!(" save error: {} ", e),
        }
    }

    fn run(&mut self) {
        loop {
            self.scroll_into_view();
            self.render();
            self.status.clear();
            let key = match read_key() { Some(k) => k, None => continue };
            match key {
                Key::Ctrl(c) if c == b'X' & 0x1F => break, // ^X
                Key::Ctrl(c) if c == b'O' & 0x1F => self.save(),
                Key::Ctrl(_) => {}
                Key::Char(c) => self.insert_char(c),
                Key::Enter   => self.enter(),
                Key::Backspace => self.backspace(),
                Key::Up => {
                    if self.cur_line > 0 { self.cur_line -= 1; self.clamp_col(); }
                }
                Key::Down => {
                    if self.cur_line + 1 < self.lines.len() {
                        self.cur_line += 1;
                        self.clamp_col();
                    }
                }
                Key::Left => {
                    if self.cur_col > 0 {
                        self.cur_col -= 1;
                    } else if self.cur_line > 0 {
                        self.cur_line -= 1;
                        self.cur_col = self.lines[self.cur_line].chars().count();
                    }
                }
                Key::Right => {
                    let len = self.lines[self.cur_line].chars().count();
                    if self.cur_col < len {
                        self.cur_col += 1;
                    } else if self.cur_line + 1 < self.lines.len() {
                        self.cur_line += 1;
                        self.cur_col = 0;
                    }
                }
                Key::Home => self.cur_col = 0,
                Key::End  => self.cur_col = self.lines[self.cur_line].chars().count(),
            }
        }
    }
}

// We're no_std-adjacent (wasi-libc) — use core::fmt with alloc.
macro_rules! alloc_format {
    ($($arg:tt)*) => {{ extern crate alloc; alloc::format!($($arg)*) }}
}
pub(crate) use alloc_format;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: nano <file>");
        std::process::exit(2);
    }
    let path = args[1].clone();
    let content = fs::read_to_string(&path).unwrap_or_default();

    let saved = save_and_raw();
    let mut ed = Editor::new(path, content);
    ed.run();
    // Clear screen + reset cursor before exit.
    let _ = std::io::stdout().write_all(b"\x1b[2J\x1b[H");
    let _ = std::io::stdout().flush();
    restore(&saved);
}
