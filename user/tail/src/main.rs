use std::collections::VecDeque;
use std::io::{BufRead, BufReader};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut n: usize = 10;
    let mut paths: Vec<String> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if a == "-n" {
            i += 1;
            if i >= args.len() {
                eprintln!("tail: -n requires an argument");
                std::process::exit(1);
            }
            n = args[i].parse().unwrap_or_else(|_| {
                eprintln!("tail: invalid count: {}", args[i]);
                std::process::exit(1);
            });
        } else if let Some(rest) = a.strip_prefix("-n") {
            n = rest.parse().unwrap_or_else(|_| {
                eprintln!("tail: invalid count: {}", rest);
                std::process::exit(1);
            });
        } else if a == "-f" {
            eprintln!("tail: -f not supported (no inotify/poll yet)");
            std::process::exit(2);
        } else if a.starts_with('-') && a.len() > 1 {
            eprintln!("tail: unknown option: {}", a);
            std::process::exit(1);
        } else {
            paths.push(a.clone());
        }
        i += 1;
    }
    let mut code = 0;
    if paths.is_empty() {
        emit(BufReader::new(std::io::stdin()), n);
    } else {
        for p in &paths {
            match std::fs::File::open(p) {
                Ok(f) => emit(BufReader::new(f), n),
                Err(e) => { eprintln!("tail: {}: {}", p, e); code = 1; }
            }
        }
    }
    std::process::exit(code);
}

fn emit<R: BufRead>(r: R, n: usize) {
    let mut buf: VecDeque<String> = VecDeque::with_capacity(n + 1);
    for line in r.lines() {
        let l = match line { Ok(s) => s, Err(_) => break, };
        buf.push_back(l);
        if buf.len() > n { buf.pop_front(); }
    }
    for l in buf { println!("{}", l); }
}
