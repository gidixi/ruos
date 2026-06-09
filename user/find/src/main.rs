//! find <path> [-name <glob>] — recursive walk, prints matching paths.
//! Glob supports `*` only (no `?`, `[abc]`). Plain substring fallback if no `*`.

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

/// True if `name` matches `pat`. `pat` may contain `*` wildcards.
fn matches(pat: &str, name: &str) -> bool {
    if pat.is_empty() { return true; }
    let parts: Vec<&str> = pat.split('*').collect();
    if parts.len() == 1 {
        return name == pat;
    }
    let mut s = name;
    // First fragment must match at start (unless empty).
    if !parts[0].is_empty() {
        if !s.starts_with(parts[0]) { return false; }
        s = &s[parts[0].len()..];
    }
    for mid in &parts[1..parts.len() - 1] {
        if mid.is_empty() { continue; }
        match s.find(mid) {
            Some(i) => s = &s[i + mid.len()..],
            None => return false,
        }
    }
    let last = parts[parts.len() - 1];
    if last.is_empty() { true } else { s.ends_with(last) }
}

fn walk(path: &str, pat: Option<&str>) {
    let leaf = path.rsplit('/').next().unwrap_or(path);
    if pat.map(|p| matches(p, leaf)).unwrap_or(true) {
        println!("{}", path);
    }
    let md = match std::fs::metadata(path) { Ok(m) => m, Err(_) => return };
    if !md.is_dir() { return; }
    for (name, _) in list(path) {
        walk(&join(path, &name), pat);
    }
}

fn main() {
    ruos_rt::init();
    let args: Vec<String> = std::env::args().collect();
    let mut root = String::from(".");
    let mut name_pat: Option<String> = None;
    let mut i = 1;
    let mut have_root = false;
    while i < args.len() {
        match args[i].as_str() {
            "-name" => {
                i += 1;
                if i >= args.len() { eprintln!("find: -name needs an argument"); std::process::exit(1); }
                name_pat = Some(args[i].clone());
            }
            s if !s.starts_with('-') && !have_root => {
                root = s.to_string();
                have_root = true;
            }
            s => { eprintln!("find: unknown arg: {}", s); std::process::exit(1); }
        }
        i += 1;
    }
    walk(&root, name_pat.as_deref());
}
