use std::fs;
use std::io::Read;

/// Print lines/words/bytes of each file (or stdin if no args).
fn main() {
    ruos_rt::init();
    let args: Vec<String> = std::env::args().collect();
    let want_l = args.iter().any(|a| a == "-l");
    let want_w = args.iter().any(|a| a == "-w");
    let want_c = args.iter().any(|a| a == "-c");
    let any_flag = want_l || want_w || want_c;

    let files: Vec<String> = args.iter().skip(1)
        .filter(|a| !a.starts_with('-')).cloned().collect();

    let mut total = (0u64, 0u64, 0u64);
    let report = |name: &str, l: u64, w: u64, b: u64| {
        let mut out = String::new();
        if !any_flag || want_l { out.push_str(&alloc::format!("{:8} ", l)); }
        if !any_flag || want_w { out.push_str(&alloc::format!("{:8} ", w)); }
        if !any_flag || want_c { out.push_str(&alloc::format!("{:8} ", b)); }
        if !name.is_empty() { out.push_str(name); }
        println!("{}", out.trim_end());
    };

    if files.is_empty() {
        let mut data = Vec::new();
        if std::io::stdin().read_to_end(&mut data).is_ok() {
            let (l, w, b) = count(&data);
            report("", l, w, b);
        }
        return;
    }
    for f in &files {
        match fs::read(f) {
            Ok(data) => {
                let (l, w, b) = count(&data);
                total = (total.0 + l, total.1 + w, total.2 + b);
                report(f, l, w, b);
            }
            Err(e) => eprintln!("wc: {}: {}", f, e),
        }
    }
    if files.len() > 1 { report("total", total.0, total.1, total.2); }
}

fn count(buf: &[u8]) -> (u64, u64, u64) {
    let mut lines = 0u64;
    let mut words = 0u64;
    let mut in_w = false;
    for &b in buf {
        if b == b'\n' { lines += 1; }
        if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
            in_w = false;
        } else if !in_w {
            in_w = true;
            words += 1;
        }
    }
    (lines, words, buf.len() as u64)
}

extern crate alloc;
