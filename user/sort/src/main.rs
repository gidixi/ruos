use std::fs;
use std::io::Read;

fn main() {
    ruos_rt::init();
    let args: Vec<String> = std::env::args().collect();
    let reverse = args.iter().any(|a| a == "-r");
    let unique  = args.iter().any(|a| a == "-u");
    let files: Vec<String> = args.iter().skip(1)
        .filter(|a| !a.starts_with('-')).cloned().collect();

    let mut data = Vec::<u8>::new();
    if files.is_empty() {
        let _ = std::io::stdin().read_to_end(&mut data);
    } else {
        for f in &files {
            match fs::read(f) {
                Ok(b) => data.extend_from_slice(&b),
                Err(e) => { eprintln!("sort: {}: {}", f, e); std::process::exit(1); }
            }
        }
    }
    let text = String::from_utf8_lossy(&data);
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last().map(|s| s.is_empty()).unwrap_or(false) { lines.pop(); }
    lines.sort();
    if reverse { lines.reverse(); }
    let mut prev: Option<&str> = None;
    for l in &lines {
        if unique {
            if prev == Some(*l) { continue; }
            prev = Some(*l);
        }
        println!("{}", l);
    }
}
