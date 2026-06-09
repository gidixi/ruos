//! wget — minimal HTTP/1.0 GET. IP-only (no DNS).
//!
//! Usage:
//!   wget http://<ip>[:<port>][/path]            -> save to ./<basename>
//!   wget -O <out> http://<ip>[:<port>][/path]   -> save to <out>
//!   wget -O - http://...                         -> stdout
//!
//! Reads until socket EOF, strips the HTTP response head (first \r\n\r\n).

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::FromRawFd;

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn tcp_dial(
        ip0: i32, ip1: i32, ip2: i32, ip3: i32,
        port: i32,
        fd_out_ptr: u32,
    ) -> i32;
}

fn parse_ip4(s: &str) -> Option<[u8; 4]> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 { return None; }
    let mut out = [0u8; 4];
    for (i, p) in parts.iter().enumerate() { out[i] = p.parse().ok()?; }
    Some(out)
}

struct Url { ip: [u8; 4], port: u16, path: String }

fn parse_url(u: &str) -> Option<Url> {
    let body = u.strip_prefix("http://").unwrap_or(u);
    let (host_port, path_part) = match body.find('/') {
        Some(i) => (&body[..i], &body[i..]),
        None    => (body, "/"),
    };
    let (host, port) = match host_port.split_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().ok()?),
        None         => (host_port, 80),
    };
    Some(Url { ip: parse_ip4(host)?, port, path: path_part.to_string() })
}

fn main() {
    ruos_rt::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut out: Option<String> = None;
    let mut url: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-O" => { out = Some(args.get(i+1).cloned().unwrap_or_default()); i += 2; }
            _    => { url = Some(args[i].clone()); i += 1; }
        }
    }
    let url = match url.and_then(|u| parse_url(&u)) {
        Some(u) => u,
        None    => { eprintln!("usage: wget [-O <out>] http://<ip>[:port][/path]"); std::process::exit(2); }
    };
    let outpath = out.unwrap_or_else(|| {
        let basename = url.path.rsplit('/').next().unwrap_or("");
        if basename.is_empty() { "index.html".to_string() } else { basename.to_string() }
    });

    let mut fd: i32 = -1;
    let r = unsafe {
        tcp_dial(
            url.ip[0] as i32, url.ip[1] as i32, url.ip[2] as i32, url.ip[3] as i32,
            url.port as i32,
            &mut fd as *mut i32 as u32,
        )
    };
    if r != 0 { eprintln!("wget: connect errno {}", r); std::process::exit(1); }

    let mut sock = unsafe { std::fs::File::from_raw_fd(fd) };
    let req = format!(
        "GET {} HTTP/1.0\r\nHost: {}.{}.{}.{}\r\nUser-Agent: ruos-wget/0.1\r\nConnection: close\r\n\r\n",
        url.path, url.ip[0], url.ip[1], url.ip[2], url.ip[3],
    );
    if let Err(e) = sock.write_all(req.as_bytes()) {
        eprintln!("wget: write: {}", e);
        std::process::exit(1);
    }
    // Read all response bytes.
    let mut all = Vec::<u8>::new();
    let mut buf = [0u8; 4096];
    loop {
        match sock.read(&mut buf) {
            Ok(0)  => break,
            Ok(n)  => all.extend_from_slice(&buf[..n]),
            Err(e) => { eprintln!("wget: read: {}", e); break; }
        }
    }
    // Strip HTTP head up to and including the first \r\n\r\n.
    let body_start = find_crlfcrlf(&all).map(|i| i + 4).unwrap_or(0);
    let body = &all[body_start..];

    if outpath == "-" {
        let _ = std::io::stdout().write_all(body);
    } else {
        match File::create(&outpath) {
            Ok(mut f) => {
                let _ = f.write_all(body);
                println!("wget: saved {} bytes to {}", body.len(), outpath);
            }
            Err(e) => { eprintln!("wget: open {}: {}", outpath, e); std::process::exit(1); }
        }
    }
}

fn find_crlfcrlf(b: &[u8]) -> Option<usize> {
    if b.len() < 4 { return None; }
    for i in 0..=b.len() - 4 {
        if &b[i..i+4] == b"\r\n\r\n" { return Some(i); }
    }
    None
}
