//! rm / rm -r: tmpfs walk via ruos_readdir (std::fs::read_dir isn't wired).

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

fn rm_recursive(path: &str) -> std::io::Result<()> {
    let md = std::fs::metadata(path)?;
    if md.is_dir() {
        for (name, _) in list(path) {
            rm_recursive(&join(path, &name))?;
        }
        std::fs::remove_dir(path)
    } else {
        std::fs::remove_file(path)
    }
}

fn main() {
    ruos_rt::init();
    let args: Vec<String> = std::env::args().collect();
    let mut recursive = false;
    let mut force = false;
    let mut paths: Vec<String> = Vec::new();
    for a in args.iter().skip(1) {
        match a.as_str() {
            "-r" | "-R" | "--recursive" => recursive = true,
            "-rf" | "-fr"               => { recursive = true; force = true; }
            "-f" | "--force"            => force = true,
            s if s.starts_with('-')     => { eprintln!("rm: unknown option: {}", s); std::process::exit(1); }
            _ => paths.push(a.clone()),
        }
    }
    if paths.is_empty() {
        eprintln!("rm: missing operand");
        std::process::exit(1);
    }
    let mut code = 0;
    for p in &paths {
        let r = if recursive {
            rm_recursive(p)
        } else {
            std::fs::remove_file(p)
        };
        if let Err(e) = r {
            if !force {
                eprintln!("rm: {}: {}", p, e);
                code = 1;
            }
        }
    }
    std::process::exit(code);
}
