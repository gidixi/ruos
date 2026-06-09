//! cp / cp -r: simple copy. Recursive walk via ruos_readdir.

use std::io::{Read, Write};

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

fn copy_file(src: &str, dst: &str) -> std::io::Result<()> {
    let mut s = std::fs::File::open(src)?;
    let mut d = std::fs::File::create(dst)?;
    let mut buf = [0u8; 4096];
    loop {
        let n = s.read(&mut buf)?;
        if n == 0 { break; }
        d.write_all(&buf[..n])?;
    }
    Ok(())
}

fn copy_recursive(src: &str, dst: &str) -> std::io::Result<()> {
    let md = std::fs::metadata(src)?;
    if md.is_dir() {
        std::fs::create_dir(dst).or_else(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => Ok(()),
            _ => Err(e),
        })?;
        for (name, _) in list(src) {
            copy_recursive(&join(src, &name), &join(dst, &name))?;
        }
        Ok(())
    } else {
        copy_file(src, dst)
    }
}

fn main() {
    ruos_rt::init();
    let args: Vec<String> = std::env::args().collect();
    let mut recursive = false;
    let mut rest: Vec<String> = Vec::new();
    for a in args.iter().skip(1) {
        match a.as_str() {
            "-r" | "-R" | "--recursive" => recursive = true,
            s if s.starts_with('-')     => { eprintln!("cp: unknown option: {}", s); std::process::exit(1); }
            _ => rest.push(a.clone()),
        }
    }
    if rest.len() != 2 {
        eprintln!("cp: usage: cp [-r] <src> <dst>");
        std::process::exit(1);
    }
    let r = if recursive { copy_recursive(&rest[0], &rest[1]) } else { copy_file(&rest[0], &rest[1]) };
    if let Err(e) = r {
        eprintln!("cp: {} -> {}: {}", rest[0], rest[1], e);
        std::process::exit(1);
    }
}
