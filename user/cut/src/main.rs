//! Minimal `cut`. Supports: -d <delim> -f <field-list> | -c <range>.

use std::fs;
use std::io::Read;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut delim = '\t';
    let mut fields: Option<Vec<usize>> = None;
    let mut chars:  Option<(usize, usize)> = None;
    let mut file:   Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-d" => { delim = args.get(i+1).and_then(|s| s.chars().next()).unwrap_or('\t'); i += 2; }
            "-f" => { fields = Some(parse_field_list(args.get(i+1).map(String::as_str).unwrap_or(""))); i += 2; }
            "-c" => { chars  = Some(parse_range(args.get(i+1).map(String::as_str).unwrap_or(""))); i += 2; }
            _    => { file = Some(a.clone()); i += 1; }
        }
    }
    let mut data = Vec::<u8>::new();
    match file {
        Some(f) => match fs::read(&f) {
            Ok(b) => data = b,
            Err(e) => { eprintln!("cut: {}: {}", f, e); std::process::exit(1); }
        },
        None => { let _ = std::io::stdin().read_to_end(&mut data); }
    }
    let text = String::from_utf8_lossy(&data);
    for line in text.split('\n') {
        if line.is_empty() && text.ends_with('\n') { continue; }
        let out: String = if let Some(fl) = &fields {
            let parts: Vec<&str> = line.split(delim).collect();
            fl.iter().filter_map(|&n| {
                if n == 0 || n > parts.len() { None } else { Some(parts[n-1]) }
            }).collect::<Vec<_>>().join(&delim.to_string())
        } else if let Some((s, e)) = chars {
            let cs: Vec<char> = line.chars().collect();
            let s = s.max(1) - 1;
            let e = e.min(cs.len());
            if s >= e { String::new() } else { cs[s..e].iter().collect() }
        } else {
            line.to_string()
        };
        println!("{}", out);
    }
}

fn parse_field_list(s: &str) -> Vec<usize> {
    s.split(',').filter_map(|t| t.parse::<usize>().ok()).collect()
}

fn parse_range(s: &str) -> (usize, usize) {
    if let Some((a, b)) = s.split_once('-') {
        let a = a.parse::<usize>().unwrap_or(1);
        let b = b.parse::<usize>().unwrap_or(usize::MAX);
        (a, b)
    } else {
        let n = s.parse::<usize>().unwrap_or(1);
        (n, n)
    }
}
