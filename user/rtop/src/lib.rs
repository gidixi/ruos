//! rtop — htop-like monitor, WASM **component** (`ruos:tui/tui-app` world).
//!
//! No WASI, no raw "ruos" module imports: system data, keys, raw mode and tty
//! output go through the `ruos:tui/host` interface (kernel-implemented); all
//! drawing goes through `ruos:tui/canvas`, satisfied at runtime by the shared
//! `tui.cwasm` provider (ratatui, AOT). Blob parsers live in `sys.rs` and are
//! unit-tested on the host.

pub mod sys;

#[cfg(target_arch = "wasm32")]
mod app {
    use crate::sys::{self, Snapshot};
    use alloc::{format, string::String, string::ToString, vec, vec::Vec};
    extern crate alloc;

    wit_bindgen::generate!({
        path: "../../wit/ruos-tui.wit",
        world: "tui-app",
    });

    use ruos::tui::canvas::{self, Rect, Color};
    use ruos::tui::host;

    const W: u16 = 80;
    const H: u16 = 24;
    const REFRESH_TICKS: i64 = 100; // 1 s at the 100 Hz tick

    fn read_snapshot() -> Option<Snapshot> {
        let cbuf = host::cpustat();
        let cpu = sys::parse_cpustat(&cbuf)?;
        let pbuf = host::proc_stat();
        let used = pbuf.len();
        let procs = sys::parse_proc_stat(&pbuf, used);
        let mbuf = host::meminfo();
        let mem = sys::parse_meminfo(&mbuf);
        let uptime_cs = host::uptime();
        Some(Snapshot { cpu, procs, mem, uptime_cs })
    }

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

    /// Sorted (cpu%, proc) rows, busiest first.
    fn sorted_rows<'s>(b: &'s Snapshot, pp: &[(u32, u16)]) -> Vec<(u16, &'s sys::Proc)> {
        let mut rows: Vec<_> = b.procs.iter().map(|p| {
            let pct = pp.iter().find(|(pid, _)| *pid == p.pid)
                .map(|(_, c)| *c).unwrap_or(0);
            (pct, p)
        }).collect();
        rows.sort_by(|x, y| y.0.cmp(&x.0));
        rows
    }

    /// --once: plain-text snapshot to the tty (used by `make run-test`).
    fn render_once(a: &Snapshot, b: &Snapshot) {
        let cp = core_pcts(a, b);
        let pp = proc_pcts(a, b);
        let mut s = String::new();
        s.push_str(&format!("rtop: uptime={}.{:02}s tasks={} cpus={}\n",
            b.uptime_cs / 100, b.uptime_cs % 100, b.procs.len(), b.cpu.cores.len()));
        let cores: Vec<String> = cp.iter().enumerate()
            .map(|(i, p)| format!("cpu{}:{}%", i, p))
            .collect();
        s.push_str(&cores.join(" "));
        s.push('\n');
        s.push_str(&format!("mem: heap {}/{} frames {}/{}\n",
            fmt_bytes(b.mem.heap_used), fmt_bytes(b.mem.heap_total),
            b.mem.frames_used, b.mem.frames_total));
        s.push_str("  PID CPU%   MEM     TIME+   CMD\n");
        for (pct, p) in sorted_rows(b, &pp) {
            s.push_str(&format!("{:>5} {:>3}  {:>7} {:>8} {}\n",
                p.pid, pct, fmt_bytes(p.mem_bytes),
                fmt_time(p.start_tick, b.uptime_cs), p.name));
        }
        host::write_tty(&s);
    }

    fn draw(a: &Snapshot, b: &Snapshot) {
        canvas::clear();
        let cp = core_pcts(a, b);
        let pp = proc_pcts(a, b);

        let header = format!(
            " rtop  uptime {}.{:02}s   tasks {}   cpus {}   (q quit)",
            b.uptime_cs / 100, b.uptime_cs % 100,
            b.procs.len(), cp.len()
        );
        canvas::draw_text(&header, Rect { x: 0, y: 0, w: W, h: 1 }, Color { r: 255, g: 255, b: 255 });

        let mut y = 1;
        for (i, pct) in cp.iter().enumerate() {
            let label = format!("cpu{:<2}", i);
            canvas::draw_text(&label, Rect { x: 0, y, w: 5, h: 1 }, Color { r: 0, g: 255, b: 0 });
            canvas::draw_bar(*pct as u8, Rect { x: 6, y, w: 36, h: 1 }, Color { r: 0, g: 255, b: 0 });
            y += 1;
        }

        let mem_pct = if b.mem.frames_total == 0 { 0 }
            else { ((b.mem.frames_used * 100) / b.mem.frames_total) as u8 };
        canvas::draw_text("mem  ", Rect { x: 0, y, w: 5, h: 1 }, Color { r: 0, g: 255, b: 255 });
        canvas::draw_bar(mem_pct.min(100), Rect { x: 6, y, w: 36, h: 1 }, Color { r: 0, g: 255, b: 255 });
        let mem_text = format!("{}/{} pages", b.mem.frames_used, b.mem.frames_total);
        canvas::draw_text(&mem_text, Rect { x: 44, y, w: 20, h: 1 }, Color { r: 0, g: 255, b: 255 });
        y += 1;

        let trows: Vec<Vec<String>> = sorted_rows(b, &pp).iter().map(|(pct, p)| {
            vec![
                format!("{}", p.pid),
                format!("{}%", pct),
                fmt_bytes(p.mem_bytes),
                fmt_time(p.start_tick, b.uptime_cs),
                p.name.clone(),
            ]
        }).collect();
        let headers = vec!["PID".to_string(), "CPU%".to_string(), "MEM".to_string(),
                           "TIME+".to_string(), "CMD".to_string()];
        let widths: Vec<u16> = vec![6, 5, 8, 9, 30];
        canvas::draw_table(&headers, &trows, &widths, Rect { x: 0, y, w: W, h: H.saturating_sub(y) });

        // Park the cursor bottom-right after every frame (it's hidden, this
        // is cosmetic) — it also gives tests a per-frame marker to count,
        // since the diff renderer repaints only changed cells.
        let mut out = canvas::flush();
        out.push_str("\x1b[24;80H");
        host::write_tty(&out);
    }

    struct App;
    export!(App);

    impl Guest for App {
        fn run(once: bool) -> i32 {
            let Some(a) = read_snapshot() else {
                host::write_tty("rtop: cpustat failed\n");
                return 1;
            };
            // ~1 s between the two snapshots so deltas are meaningful.
            // poll-key doubles as the sleep; on a tty-less/BSP run it returns
            // EOF immediately and the deltas are just zero (still renders).
            let _ = host::poll_key(REFRESH_TICKS);
            let Some(b) = read_snapshot() else {
                host::write_tty("rtop: cpustat failed\n");
                return 1;
            };

            if once {
                render_once(&a, &b);
                return 0;
            }

            host::set_raw(true);
            host::write_tty("\x1b[?1049h\x1b[?25l\x1b[2J");
            canvas::init(W, H);

            let mut prev = a;
            let mut cur = b;
            loop {
                draw(&prev, &cur);
                let k = host::poll_key(REFRESH_TICKS);
                if k < 0 { break; }                      // stdin EOF / no tty
                if k >= 1 {
                    let ch = (k - 1) as u8;
                    if ch == b'q' || ch == 3 { break; }  // 'q' or Ctrl-C
                }
                prev = cur;
                cur = match read_snapshot() {
                    Some(s) => s,
                    None => break,
                };
            }

            // Restore the terminal: leave alt screen, show cursor, cooked mode.
            host::write_tty("\x1b[?25h\x1b[?1049l");
            host::set_raw(false);
            0
        }
    }
}
