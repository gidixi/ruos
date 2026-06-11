//! Shared TUI provider component (`tui.cwasm`): exports `ruos:tui/canvas`.
//!
//! All widgets render into ONE ratatui back `Buffer` (immediate-mode calls
//! accumulate, they don't fight each other); `flush()` diffs the back buffer
//! against the last-flushed one and RETURNS the ANSI string — this component
//! has no I/O (wasm32-unknown-unknown, no WASI). The app writes the string to
//! its tty via `ruos:tui/host.write-tty`.

#![allow(warnings)]
wit_bindgen::generate!({
    path: "../../wit/ruos-tui.wit",
    world: "tui-provider",
});

use exports::ruos::tui::canvas::{Guest, Rect as WRect, Color as WColor};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Paragraph, Row, Table, Cell as TCell, Widget};

struct TuiState {
    back: Buffer,
    front: Buffer,
    /// Force a full repaint on next flush (first frame, after init/clear).
    full: bool,
}

/// Singleton: wasm guest is single-threaded, one terminal per app instance.
static mut STATE: Option<TuiState> = None;

fn state() -> &'static mut TuiState {
    unsafe {
        let s = &mut *core::ptr::addr_of_mut!(STATE);
        s.get_or_insert_with(|| {
            let area = Rect::new(0, 0, 80, 24);
            TuiState { back: Buffer::empty(area), front: Buffer::empty(area), full: true }
        })
    }
}

fn map_color(c: &WColor) -> Color { Color::Rgb(c.r, c.g, c.b) }

/// Clip a WIT rect to the buffer area (ratatui Buffer indexing panics OOB).
fn clip(area: &WRect, buf: &Buffer) -> Option<Rect> {
    let r = Rect::new(area.x, area.y, area.w, area.h).intersection(buf.area);
    if r.width == 0 || r.height == 0 { None } else { Some(r) }
}

fn sgr_for(cell: &Cell, s: &mut String) {
    use core::fmt::Write as _;
    s.push_str("\x1b[0m");
    if let Color::Rgb(r, g, b) = cell.fg {
        let _ = write!(s, "\x1b[38;2;{};{};{}m", r, g, b);
    }
    if let Color::Rgb(r, g, b) = cell.bg {
        let _ = write!(s, "\x1b[48;2;{};{};{}m", r, g, b);
    }
    if cell.modifier.contains(Modifier::BOLD) { s.push_str("\x1b[1m"); }
    if cell.modifier.contains(Modifier::REVERSED) { s.push_str("\x1b[7m"); }
}

/// Emit ANSI for a list of (x, y, cell) updates, merging runs of adjacent
/// same-style cells on a row to cut cursor-move/SGR escapes.
fn emit(updates: &[(u16, u16, &Cell)]) -> String {
    use core::fmt::Write as _;
    let mut s = String::new();
    let mut last: Option<(u16, u16, Style)> = None;
    for &(x, y, cell) in updates {
        let style = Style::default().fg(cell.fg).bg(cell.bg).add_modifier(cell.modifier);
        let contiguous = matches!(last, Some((lx, ly, ls)) if ly == y && lx + 1 == x && ls == style);
        if !contiguous {
            let _ = write!(s, "\x1b[{};{}H", y + 1, x + 1);
            sgr_for(cell, &mut s);
        }
        s.push_str(cell.symbol());
        last = Some((x, y, style));
    }
    if !updates.is_empty() { s.push_str("\x1b[0m"); }
    s
}

struct Component;
export!(Component);

impl Guest for Component {
    fn init(width: u16, height: u16) {
        let area = Rect::new(0, 0, width.max(1), height.max(1));
        let st = state();
        st.back = Buffer::empty(area);
        st.front = Buffer::empty(area);
        st.full = true;
    }

    fn clear() {
        let st = state();
        let area = st.back.area;
        st.back = Buffer::empty(area);
    }

    fn draw_text(text: String, area: WRect, fg: WColor) {
        let st = state();
        if let Some(r) = clip(&area, &st.back) {
            Paragraph::new(text)
                .style(Style::default().fg(map_color(&fg)))
                .render(r, &mut st.back);
        }
    }

    fn draw_bar(pct: u8, area: WRect, fg: WColor) {
        let st = state();
        if let Some(r) = clip(&area, &st.back) {
            let pct = pct.min(100);
            // "[###---] NN%" — bar fills the width minus brackets and label.
            let width = (r.width as usize).saturating_sub(7);
            let fill = pct as usize * width / 100;
            let mut bar = String::with_capacity(width + 8);
            bar.push('[');
            for i in 0..width { bar.push(if i < fill { '#' } else { '-' }); }
            bar.push(']');
            let text = format!("{} {:>3}%", bar, pct);
            Paragraph::new(text)
                .style(Style::default().fg(map_color(&fg)))
                .render(r, &mut st.back);
        }
    }

    fn draw_table(headers: Vec<String>, rows: Vec<Vec<String>>, widths: Vec<u16>, area: WRect) {
        let st = state();
        if let Some(r) = clip(&area, &st.back) {
            let header = Row::new(headers.into_iter().map(TCell::from).collect::<Vec<_>>())
                .style(Style::default().add_modifier(Modifier::REVERSED));
            let trows: Vec<Row> = rows.into_iter()
                .map(|row| Row::new(row.into_iter().map(TCell::from).collect::<Vec<_>>()))
                .collect();
            let constraints: Vec<ratatui::layout::Constraint> = widths.into_iter()
                .map(ratatui::layout::Constraint::Length)
                .collect();
            Table::new(trows, constraints)
                .header(header)
                .column_spacing(1)
                .render(r, &mut st.back);
        }
    }

    fn flush() -> String {
        let st = state();
        let mut out = String::new();
        if st.full {
            st.full = false;
            out.push_str("\x1b[2J");
            let area = st.back.area;
            let all: Vec<(u16, u16, &Cell)> = area.positions()
                .map(|p| (p.x, p.y, &st.back[(p.x, p.y)]))
                .collect();
            out.push_str(&emit(&all));
        } else {
            let updates = st.front.diff(&st.back);
            out.push_str(&emit(&updates));
        }
        st.front = st.back.clone();
        out
    }
}
