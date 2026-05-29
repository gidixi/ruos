use std::fs;
use std::io::Read;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let count = args.iter().any(|a| a == "-c");
    let file: Option<&String> = args.iter().skip(1).find(|a| !a.starts_with('-'));
    let mut data = Vec::<u8>::new();
    match file {
        Some(f) => match fs::read(f) {
            Ok(b) => data = b,
            Err(e) => { eprintln!("uniq: {}: {}", f, e); std::process::exit(1); }
        },
        None => { let _ = std::io::stdin().read_to_end(&mut data); }
    }
    let text = String::from_utf8_lossy(&data);
    let mut prev: Option<String> = None;
    let mut cnt: u64 = 0;
    for line in text.split('\n') {
        if Some(line) == prev.as_deref() {
            cnt += 1;
        } else {
            if let Some(p) = prev.as_ref() {
                if count { println!("{:>7} {}", cnt, p); } else { println!("{}", p); }
            }
            prev = Some(line.to_string());
            cnt = 1;
        }
    }
    if let Some(p) = prev.as_ref() {
        if !p.is_empty() {
            if count { println!("{:>7} {}", cnt, p); } else { println!("{}", p); }
        }
    }
}
