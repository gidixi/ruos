//! grep -rn pattern [paths...]. Plain substring match (no regex).

use std::io::{BufRead, BufReader};

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn readdir(p: u32, pl: u32, b: u32, bl: u32, n: u32) -> i32;
}

fn list(path: &str) -> Vec<(String, bool)> {
    let mut buf = vec![0u8; 16384];
    let mut n: u32 = 0;
    let errno = unsafe {
        readdir(path.as_ptr() as u32, path.len() as u32,
                buf.as_mut_ptr() as u32, buf.len() as u32,
                &mut n as *mut u32 as u32)
    };
    let mut out = Vec::new();
    if errno != 0 { return out; }
    let mut o = 0usize;
    while o + 12 <= n as usize {
        let kind = buf[o];
        let nl = u16::from_le_bytes([buf[o + 2], buf[o + 3]]) as usize;
        o += 12;
        if o + nl > n as usize { break; }
        let name = std::str::from_utf8(&buf[o..o + nl]).unwrap_or("?").to_string();
        o += nl;
        out.push((name, kind == 1));
    }
    out
}

fn join(dir: &str, name: &str) -> String {
    let mut s = String::from(dir);
    if !s.ends_with('/') { s.push('/'); }
    s.push_str(name);
    s
}

fn scan_file(path: &str, pat: &str, show_name: bool, show_num: bool) -> bool {
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut matched = false;
    for (i, line) in BufReader::new(f).lines().enumerate() {
        let l = match line { Ok(s) => s, Err(_) => break, };
        if l.contains(pat) {
            matched = true;
            let mut out = String::new();
            if show_name { out.push_str(path); out.push(':'); }
            if show_num  { out.push_str(&format!("{}:", i + 1)); }
            out.push_str(&l);
            println!("{}", out);
        }
    }
    matched
}

fn walk(path: &str, pat: &str, recursive: bool, show_num: bool) {
    let md = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => { eprintln!("grep: {}: {}", path, e); return; }
    };
    if md.is_dir() {
        if !recursive {
            eprintln!("grep: {}: is a directory", path);
            return;
        }
        for (name, _) in list(path) {
            walk(&join(path, &name), pat, recursive, show_num);
        }
    } else {
        scan_file(path, pat, true, show_num);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut recursive = false;
    let mut show_num = false;
    let mut rest: Vec<String> = Vec::new();
    for a in args.iter().skip(1) {
        match a.as_str() {
            "-r" | "-R" | "--recursive" => recursive = true,
            "-n"                        => show_num = true,
            "-rn" | "-nr"               => { recursive = true; show_num = true; }
            s if s.starts_with('-')     => { eprintln!("grep: unknown option: {}", s); std::process::exit(2); }
            _ => rest.push(a.clone()),
        }
    }
    if rest.is_empty() {
        eprintln!("grep: usage: grep [-rn] <pattern> [path ...]");
        std::process::exit(2);
    }
    let pat = rest.remove(0);
    if rest.is_empty() {
        scan_file("/dev/stdin", &pat, false, show_num);
    } else {
        for p in rest {
            walk(&p, &pat, recursive, show_num);
        }
    }
}
