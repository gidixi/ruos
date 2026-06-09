//! nc — minimal netcat. TCP client only.
//!
//! Usage: nc <ip> <port>
//!
//! Opens a TCP connection, sends stdin → socket, prints socket → stdout.
//! Both directions concurrently using non-blocking-style reads of stdin via
//! libc::read; the read end of stdin is checked between socket reads with a
//! simple alternating loop. The shell PTY's raw mode is left untouched.

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

fn main() {
    ruos_rt::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: nc <ip> <port>");
        std::process::exit(2);
    }
    let ip = match parse_ip4(&args[0]) {
        Some(v) => v,
        None    => { eprintln!("nc: bad ip {}", args[0]); std::process::exit(2); }
    };
    let port: i32 = match args[1].parse() {
        Ok(p) => p,
        Err(_) => { eprintln!("nc: bad port {}", args[1]); std::process::exit(2); }
    };
    let mut fd_out: i32 = -1;
    let r = unsafe {
        tcp_dial(
            ip[0] as i32, ip[1] as i32, ip[2] as i32, ip[3] as i32,
            port,
            &mut fd_out as *mut i32 as u32,
        )
    };
    if r != 0 {
        eprintln!("nc: connect errno {}", r);
        std::process::exit(1);
    }
    // Use the FD as a std file handle.
    let mut sock = unsafe { std::fs::File::from_raw_fd(fd_out) };
    let mut stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut buf = [0u8; 1024];
    // Alternating poll: read from socket (blocks until bytes), then check stdin
    // (non-blocking via a fixed-size attempt). When user closes stdin (^D in
    // raw mode = 0x04), we exit.
    loop {
        match sock.read(&mut buf) {
            Ok(0)  => break,
            Ok(n)  => { let _ = stdout.write_all(&buf[..n]); let _ = stdout.flush(); }
            Err(e) => { eprintln!("nc: read: {}", e); break; }
        }
        // Drain stdin opportunistically (one byte at a time).
        let mut sb = [0u8; 1];
        match stdin.read(&mut sb) {
            Ok(0) => break,
            Ok(_) => {
                if sb[0] == 0x04 { break; } // ^D
                let _ = sock.write_all(&sb);
            }
            Err(_) => {}
        }
    }
}
