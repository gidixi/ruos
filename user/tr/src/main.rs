//! Minimal `tr`. Supports: tr SET1 SET2  (1:1 char map) and tr -d SET (delete).

use std::io::Read;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: tr SET1 SET2 | tr -d SET");
        std::process::exit(2);
    }
    let delete = args[0] == "-d";
    let (set1, set2): (String, String) = if delete {
        if args.len() < 2 { eprintln!("tr: missing SET"); std::process::exit(2); }
        (args[1].clone(), String::new())
    } else {
        if args.len() < 2 { eprintln!("tr: missing SET2"); std::process::exit(2); }
        (args[0].clone(), args[1].clone())
    };
    let mut data = Vec::<u8>::new();
    let _ = std::io::stdin().read_to_end(&mut data);
    let mut out = String::new();
    let set1c: Vec<char> = set1.chars().collect();
    let set2c: Vec<char> = set2.chars().collect();
    for c in String::from_utf8_lossy(&data).chars() {
        if let Some(idx) = set1c.iter().position(|&x| x == c) {
            if delete { continue; }
            let to = set2c.get(idx).copied().unwrap_or(*set2c.last().unwrap_or(&c));
            out.push(to);
        } else {
            out.push(c);
        }
    }
    print!("{}", out);
}
