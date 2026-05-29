//! du [-s] [-h] <path> — sum of file sizes (directories: 0). With -s prints
//! only the total. With -h prints with K/M/G suffix.

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn readdir(p: u32, pl: u32, b: u32, bl: u32, n: u32) -> i32;
}

fn list(path: &str) -> Vec<(String, bool, u64)> {
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
        let size = u64::from_le_bytes([
            buf[o + 4], buf[o + 5], buf[o + 6], buf[o + 7],
            buf[o + 8], buf[o + 9], buf[o + 10], buf[o + 11],
        ]);
        o += 12;
        if o + nl > n as usize { break; }
        let name = std::str::from_utf8(&buf[o..o + nl]).unwrap_or("?").to_string();
        o += nl;
        out.push((name, kind == 1, size));
    }
    out
}

fn join(dir: &str, name: &str) -> String {
    let mut s = String::from(dir);
    if !s.ends_with('/') { s.push('/'); }
    s.push_str(name);
    s
}

fn total(path: &str) -> u64 {
    let md = match std::fs::metadata(path) { Ok(m) => m, Err(_) => return 0 };
    if !md.is_dir() {
        return md.len();
    }
    let mut sum = 0u64;
    for (name, is_dir, size) in list(path) {
        if is_dir {
            sum += total(&join(path, &name));
        } else {
            sum += size;
        }
    }
    sum
}

fn human(b: u64) -> String {
    let units = ["B", "K", "M", "G", "T"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 { v /= 1024.0; i += 1; }
    if i == 0 { format!("{}{}", b, units[0]) }
    else      { format!("{:.1}{}", v, units[i]) }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut summary = false;
    let mut human_r = false;
    let mut paths: Vec<String> = Vec::new();
    for a in args.iter().skip(1) {
        match a.as_str() {
            "-s" => summary = true,
            "-h" => human_r = true,
            "-sh" | "-hs" => { summary = true; human_r = true; }
            s if s.starts_with('-') => { eprintln!("du: unknown option: {}", s); std::process::exit(1); }
            _ => paths.push(a.clone()),
        }
    }
    if paths.is_empty() { paths.push(".".to_string()); }
    let _ = summary; // summary is the only behavior we support (-s default)
    for p in &paths {
        let t = total(p);
        let s = if human_r { human(t) } else { format!("{}", t) };
        println!("{}\t{}", s, p);
    }
}
