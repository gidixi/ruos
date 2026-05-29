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
                eprintln!("head: -n requires an argument");
                std::process::exit(1);
            }
            n = args[i].parse().unwrap_or_else(|_| {
                eprintln!("head: invalid count: {}", args[i]);
                std::process::exit(1);
            });
        } else if let Some(rest) = a.strip_prefix("-n") {
            n = rest.parse().unwrap_or_else(|_| {
                eprintln!("head: invalid count: {}", rest);
                std::process::exit(1);
            });
        } else if a.starts_with('-') && a.len() > 1 {
            eprintln!("head: unknown option: {}", a);
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
                Err(e) => { eprintln!("head: {}: {}", p, e); code = 1; }
            }
        }
    }
    std::process::exit(code);
}

fn emit<R: BufRead>(r: R, n: usize) {
    for (i, line) in r.lines().enumerate() {
        if i >= n { break; }
        match line { Ok(l) => println!("{}", l), Err(_) => break, }
    }
}
