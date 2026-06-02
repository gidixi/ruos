mod sys;
mod ansi_backend;
mod raw;

use sys::Snapshot;
use ratatui::Terminal;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style, Modifier};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Row, Table, Cell as TCell};
use ansi_backend::AnsiBackend;

const W: u16 = 80;
const H: u16 = 24;

/// ASCII progress bar `[####------]` — the ruos framebuffer font has no Unicode
/// block/box glyphs (they render as '?'), so the TUI stays pure ASCII.
fn ascii_bar(pct: u16, width: usize) -> String {
    let fill = (pct as usize * width / 100).min(width);
    let mut s = String::with_capacity(width + 2);
    s.push('[');
    for i in 0..width { s.push(if i < fill { '#' } else { '-' }); }
    s.push(']');
    s
}

// ---------------------------------------------------------------------------
// sys_read: wasm32 delegates to sys::read_snapshot; host build returns None.
// This avoids a name conflict between `use sys::read_snapshot` and the local
// stub that the host build needs.
// ---------------------------------------------------------------------------
#[cfg(target_arch = "wasm32")]
fn sys_read() -> Option<Snapshot> {
    sys::read_snapshot()
}

#[cfg(not(target_arch = "wasm32"))]
fn sys_read() -> Option<Snapshot> {
    None
}

// ---------------------------------------------------------------------------
// CPU / process utilities
// ---------------------------------------------------------------------------

fn core_pcts(a: &Snapshot, b: &Snapshot) -> Vec<u16> {
    let n = a.cpu.cores.len().min(b.cpu.cores.len());
    let mut v = Vec::with_capacity(n);
    for i in 0..n {
        let db = b.cpu.cores[i].busy.saturating_sub(a.cpu.cores[i].busy);
        let di = b.cpu.cores[i].idle.saturating_sub(a.cpu.cores[i].idle);
        let tot = db + di;
        v.push(if tot == 0 { 0 } else { ((db * 100) / tot) as u16 });
    }
    v
}

fn proc_pcts(a: &Snapshot, b: &Snapshot) -> Vec<(u32, u16)> {
    let wall: u64 = (0..a.cpu.cores.len().min(b.cpu.cores.len()))
        .map(|i| {
            b.cpu.cores[i].busy.saturating_sub(a.cpu.cores[i].busy)
                + b.cpu.cores[i].idle.saturating_sub(a.cpu.cores[i].idle)
        })
        .sum();
    let mut out = Vec::new();
    for pb in &b.procs {
        let prev = a.procs.iter().find(|p| p.pid == pb.pid)
            .map(|p| p.cpu_tsc).unwrap_or(0);
        let d = pb.cpu_tsc.saturating_sub(prev);
        let pct = if wall == 0 { 0 } else { ((d * 100) / wall).min(100) as u16 };
        out.push((pb.pid, pct));
    }
    out
}

fn fmt_bytes(b: u64) -> String {
    if b >= 1 << 20 { format!("{}M", b >> 20) }
    else if b >= 1 << 10 { format!("{}K", b >> 10) }
    else { format!("{}B", b) }
}

fn fmt_time(start_cs: u64, now_cs: u64) -> String {
    let cs = now_cs.saturating_sub(start_cs);
    format!("{}:{:02}.{:02}", cs / 6000, (cs / 100) % 60, cs % 100)
}

// ---------------------------------------------------------------------------
// --once: plain text output (grep-safe, tested path)
// First line must match: `rtop: uptime=`
// Second line must contain `cpuN:%` tokens
// ---------------------------------------------------------------------------

fn render_once(a: &Snapshot, b: &Snapshot) {
    let cp = core_pcts(a, b);
    let pp = proc_pcts(a, b);
    println!("rtop: uptime={}.{:02}s tasks={} cpus={}",
        b.uptime_cs / 100, b.uptime_cs % 100, b.procs.len(), b.cpu.cores.len());
    let cores: Vec<String> = cp.iter().enumerate()
        .map(|(i, p)| format!("cpu{}:{}%", i, p))
        .collect();
    println!("{}", cores.join(" "));
    println!("mem: heap {}/{} frames {}/{}",
        fmt_bytes(b.mem.heap_used), fmt_bytes(b.mem.heap_total),
        b.mem.frames_used, b.mem.frames_total);
    println!("  PID CPU%   MEM     TIME+   CMD");
    let mut rows: Vec<_> = b.procs.iter().map(|p| {
        let pct = pp.iter().find(|(pid, _)| *pid == p.pid)
            .map(|(_, c)| *c).unwrap_or(0);
        (pct, p)
    }).collect();
    rows.sort_by(|x, y| y.0.cmp(&x.0));
    for (pct, p) in rows {
        println!("{:>5} {:>3}  {:>7} {:>8} {}",
            p.pid, pct, fmt_bytes(p.mem_bytes),
            fmt_time(p.start_tick, b.uptime_cs), p.name);
    }
}

// ---------------------------------------------------------------------------
// Interactive TUI (ratatui 0.29 + AnsiBackend)
// ---------------------------------------------------------------------------

fn draw(term: &mut Terminal<AnsiBackend>, a: &Snapshot, b: &Snapshot) {
    let cp = core_pcts(a, b);
    let pp = proc_pcts(a, b);
    let _ = term.draw(|f| {
        let area = f.area();
        let ncore = cp.len() as u16;
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(ncore.max(1)),
                Constraint::Length(1),
                Constraint::Min(3),
            ])
            .split(area);

        let header = Line::from(format!(
            " rtop  uptime {}.{:02}s   tasks {}   cpus {}   (q quit)",
            b.uptime_cs / 100, b.uptime_cs % 100,
            b.procs.len(), cp.len()
        ));
        f.render_widget(
            Paragraph::new(header)
                .style(Style::default().add_modifier(Modifier::BOLD)),
            chunks[0],
        );

        // Per-core CPU gauges
        let core_rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints(vec![Constraint::Length(1); cp.len().max(1)])
            .split(chunks[1]);
        for (i, pct) in cp.iter().enumerate() {
            f.render_widget(
                Paragraph::new(format!("cpu{:<2} {} {:>3}%", i, ascii_bar(*pct, 30), pct))
                    .style(Style::default().fg(Color::Green)),
                core_rows[i],
            );
        }

        // Memory bar (ASCII)
        let mem_pct = if b.mem.frames_total == 0 {
            0
        } else {
            ((b.mem.frames_used * 100) / b.mem.frames_total) as u16
        };
        f.render_widget(
            Paragraph::new(format!(
                "mem {} {}/{} pages",
                ascii_bar(mem_pct.min(100), 30), b.mem.frames_used, b.mem.frames_total
            ))
            .style(Style::default().fg(Color::Cyan)),
            chunks[2],
        );

        // Process table
        let mut rows: Vec<_> = b.procs.iter().map(|p| {
            let pct = pp.iter().find(|(pid, _)| *pid == p.pid)
                .map(|(_, c)| *c).unwrap_or(0);
            (pct, p)
        }).collect();
        rows.sort_by(|x, y| y.0.cmp(&x.0));
        let trows: Vec<Row> = rows.iter().map(|(pct, p)| {
            Row::new(vec![
                TCell::from(format!("{}", p.pid)),
                TCell::from(format!("{}%", pct)),
                TCell::from(fmt_bytes(p.mem_bytes)),
                TCell::from(fmt_time(p.start_tick, b.uptime_cs)),
                TCell::from(p.name.clone()),
            ])
        }).collect();
        let widths = [
            Constraint::Length(6),
            Constraint::Length(5),
            Constraint::Length(8),
            Constraint::Length(9),
            Constraint::Min(10),
        ];
        // No box border (the framebuffer font lacks '─'); a reverse-video header
        // row (ANSI, ASCII-safe) separates it instead.
        let table = Table::new(trows, widths)
            .header(
                Row::new(vec!["PID", "CPU%", "MEM", "TIME+", "CMD"])
                    .style(Style::default().add_modifier(Modifier::REVERSED)),
            );
        f.render_widget(table, chunks[3]);
    });
}

fn sleep_ms(ms: u64) {
    std::thread::sleep(std::time::Duration::from_millis(ms));
}

fn main() {
    let once = std::env::args().any(|a| a == "--once");

    let a = match sys_read() {
        Some(s) => s,
        None => { eprintln!("rtop: cpustat failed"); return; }
    };
    sleep_ms(1000);
    let b = match sys_read() {
        Some(s) => s,
        None => { eprintln!("rtop: cpustat failed"); return; }
    };

    if once {
        render_once(&a, &b);
        return;
    }

    // Interactive mode — raw terminal + ratatui TUI.
    // stdin().read() in raw mode blocks until a key is pressed.
    // We sleep 1 s between redraws; the user pressing 'q' or Ctrl-C exits.
    let _guard = raw::TermGuard::enter();
    let backend = AnsiBackend::new(W, H);
    let mut term = Terminal::new(backend).expect("terminal");
    let _ = term.clear();

    let mut prev = a;
    let mut cur = b;
    loop {
        draw(&mut term, &prev, &cur);
        // Wait up to ~1 s for a key via the ruos poll_stdin host fn, which
        // races the read against a timer: a keystroke returns immediately
        // (instant 'q'), a timeout returns so we redraw (htop-style 1 Hz
        // auto-refresh), and EOF (e.g. the SSH session closed) ends the loop.
        let (code, ch) = poll_key(REFRESH_TICKS);
        if code < 0 { break; }                              // stdin EOF
        if code == 1 && (ch == b'q' || ch == 3) { break; }  // 'q' or Ctrl-C
        prev = cur;
        cur = match sys_read() {
            Some(s) => s,
            None => break,
        };
    }
}

/// Refresh cadence in 100 Hz timer ticks (100 ticks = 1 s).
const REFRESH_TICKS: i64 = 100;

#[cfg(target_arch = "wasm32")]
fn poll_key(t: i64) -> (i32, u8) { sys::poll_key(t) }
#[cfg(not(target_arch = "wasm32"))]
fn poll_key(_t: i64) -> (i32, u8) { (-1, 0) } // host stub (interactive path is wasm-only)
