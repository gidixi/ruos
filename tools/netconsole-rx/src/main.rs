//! netconsole-rx — host-side receiver for ruos netconsole.
//!
//! ruos, built with `--features netconsole`, broadcasts every kernel log line as
//! a UDP datagram to `255.255.255.255:6666`. This is a zero-dependency, cross
//! platform (Windows/Linux/macOS) drop-in for `nc -ul 6666`: it binds the port,
//! receives those datagrams and writes their bytes straight to stdout. Each
//! datagram is already a whole log line (the kernel cuts on `\n`), so the output
//! reads exactly like the serial console would.
//!
//! Everything written to stdout is also mirrored to `netconsole.log` in the same
//! directory as the executable; that file is TRUNCATED on every start, so it
//! always holds just the current session.
//!
//! Usage:
//!   netconsole-rx                 # listen on 0.0.0.0:6666
//!   netconsole-rx -p 7000         # custom port
//!   netconsole-rx --bind 0.0.0.0  # custom bind address
//!   netconsole-rx --src           # prefix each datagram with its source IP
//!   netconsole-rx -h              # help
//!
//! Note (Windows): the first run may pop a Windows Defender Firewall prompt for
//! inbound UDP — allow it on the private/LAN profile, or the datagrams are
//! dropped before they reach this process.

use std::fs::File;
use std::io::{self, Write};
use std::net::UdpSocket;
use std::path::PathBuf;
use std::process::ExitCode;

const DEFAULT_PORT: u16 = 6666;
const DEFAULT_BIND: &str = "0.0.0.0";
/// Log file name, written next to the executable. Truncated on every start.
const LOG_NAME: &str = "netconsole.log";

/// Path of `netconsole.log` next to the running executable; falls back to the
/// current working directory if the exe path can't be resolved.
fn log_path() -> PathBuf {
    match std::env::current_exe() {
        Ok(exe) => match exe.parent() {
            Some(dir) => dir.join(LOG_NAME),
            None => PathBuf::from(LOG_NAME),
        },
        Err(_) => PathBuf::from(LOG_NAME),
    }
}

struct Args {
    port: u16,
    bind: String,
    show_src: bool,
}

fn print_help() {
    eprintln!(
        "netconsole-rx — receive ruos netconsole UDP broadcast logs\n\
         \n\
         USAGE:\n    netconsole-rx [OPTIONS]\n\
         \n\
         OPTIONS:\n\
         \x20   -p, --port <PORT>   UDP port to listen on (default {DEFAULT_PORT})\n\
         \x20       --bind <ADDR>   bind address (default {DEFAULT_BIND})\n\
         \x20       --src           prefix each line with the sender's IP\n\
         \x20   -h, --help          show this help\n\
         \n\
         Drop-in for `nc -ul {DEFAULT_PORT}`. Build ruos with --features netconsole."
    );
}

/// Parse argv. Returns Err with a message on bad input, Ok(None) when help was
/// requested (caller should exit 0).
fn parse_args() -> Result<Option<Args>, String> {
    let mut args = Args { port: DEFAULT_PORT, bind: DEFAULT_BIND.to_string(), show_src: false };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => {
                print_help();
                return Ok(None);
            }
            "-p" | "--port" => {
                let v = it.next().ok_or_else(|| format!("{a} requires a value"))?;
                args.port = v.parse().map_err(|_| format!("invalid port: {v}"))?;
            }
            "--bind" => {
                args.bind = it.next().ok_or_else(|| "--bind requires a value".to_string())?;
            }
            "--src" => args.show_src = true,
            other => return Err(format!("unknown argument: {other} (try --help)")),
        }
    }
    Ok(Some(args))
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(Some(a)) => a,
        Ok(None) => return ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    let addr = format!("{}:{}", args.bind, args.port);
    let socket = match UdpSocket::bind(&addr) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot bind {addr}: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Log file next to the exe, TRUNCATED on every start (File::create wipes any
    // previous run's contents). Mirrors everything also written to stdout.
    let log = log_path();
    let mut logf = match File::create(&log) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: cannot create log file {}: {e}", log.display());
            return ExitCode::FAILURE;
        }
    };

    eprintln!("netconsole-rx: listening on {addr} (UDP) — Ctrl-C to stop");
    eprintln!("netconsole-rx: logging to {} (cleared on each start)", log.display());

    // 64 KiB covers the largest plausible datagram; netconsole caps at ~512 B.
    let mut buf = [0u8; 64 * 1024];
    let stdout = io::stdout();
    loop {
        match socket.recv_from(&mut buf) {
            Ok((n, src)) => {
                // Compose the line once, then mirror it to stdout and the log.
                let mut line: Vec<u8> = Vec::with_capacity(n + 24);
                if args.show_src {
                    line.extend_from_slice(format!("[{}] ", src.ip()).as_bytes());
                }
                line.extend_from_slice(&buf[..n]);
                // netconsole datagrams end on a newline; if one didn't, add one
                // so prefixes and shells stay line-aligned.
                if line.last() != Some(&b'\n') {
                    line.push(b'\n');
                }

                let mut out = stdout.lock();
                let _ = out.write_all(&line);
                let _ = out.flush();
                let _ = logf.write_all(&line);
                let _ = logf.flush();
            }
            Err(e) => {
                eprintln!("netconsole-rx: recv error: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
}
