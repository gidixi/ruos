//! `service` — minimal service manager CLI. Wraps the kernel registry
//! exposed by `ruos_service_list` / `_start` / `_status`.
//!
//! Subcommands (positional, no flags):
//!   service [list]        — pretty-print the registry
//!   service start <name>  — queue a start request
//!   service status <name> — one-line status summary
//!   service stop <name>   — reserved; not implemented in MVP
//!
//! Exit codes: 0 success; 1 user-facing error (NotFound, errno!=0);
//! 2 usage error; 3 NotSupported (stop).

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn service_list(buf_ptr: u32, buf_len: u32, used_ptr: u32) -> i32;
    fn service_start(name_ptr: u32, name_len: u32) -> i32;
    fn service_status(
        name_ptr: u32, name_len: u32,
        buf_ptr: u32, buf_len: u32, used_ptr: u32,
    ) -> i32;
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let sub = args.first().map(String::as_str).unwrap_or("list");

    match sub {
        "list" => cmd_list(),
        "start" => match args.get(1) {
            Some(name) => cmd_start(name),
            None       => usage_err("start: missing service name"),
        },
        "status" => match args.get(1) {
            Some(name) => cmd_status(name),
            None       => usage_err("status: missing service name"),
        },
        "stop" => {
            eprintln!("stop: not implemented in MVP — kill the fiber via PTY shutdown (TODO)");
            std::process::exit(3);
        }
        other => usage_err(&format!("unknown subcommand: {}", other)),
    }
}

fn cmd_list() {
    let text = match fetch_list() {
        Some(s) => s,
        None    => std::process::exit(1),
    };
    println!("{:<12} {:<10} {:>5}  {:>5}  {}", "NAME", "STATUS", "PID", "RUNS", "PATH");
    for line in text.lines() {
        if line.is_empty() { continue; }
        let mut it = line.split('\t');
        let name = it.next().unwrap_or("");
        let stat = it.next().unwrap_or("");
        let pid  = it.next().unwrap_or("-");
        let runs = it.next().unwrap_or("0");
        let path = it.next().unwrap_or("");
        println!("{:<12} {:<10} {:>5}  {:>5}  {}", name, stat, pid, runs, path);
    }
}

fn fetch_list() -> Option<String> {
    let mut buf = vec![0u8; 8192];
    let mut used: u32 = 0;
    let errno = unsafe {
        service_list(
            buf.as_mut_ptr() as u32,
            buf.len() as u32,
            &mut used as *mut u32 as u32,
        )
    };
    if errno != 0 {
        eprintln!("service list: errno {}", errno);
        return None;
    }
    let n = (used as usize).min(buf.len());
    Some(String::from_utf8_lossy(&buf[..n]).into_owned())
}

fn cmd_start(name: &str) {
    let bytes = name.as_bytes();
    let errno = unsafe {
        service_start(bytes.as_ptr() as u32, bytes.len() as u32)
    };
    match errno {
        0  => println!("service: {}: started", name),
        1  => { eprintln!("service: {}: not found", name);        std::process::exit(1); }
        2  => { eprintln!("service: {}: already running", name);  std::process::exit(1); }
        3  => { eprintln!("service: {}: not supported", name);    std::process::exit(1); }
        n  => { eprintln!("service: {}: errno {}", name, n);       std::process::exit(1); }
    }
}

fn cmd_status(name: &str) {
    let mut buf = vec![0u8; 4096];
    let mut used: u32 = 0;
    let nb = name.as_bytes();
    let errno = unsafe {
        service_status(
            nb.as_ptr() as u32, nb.len() as u32,
            buf.as_mut_ptr() as u32, buf.len() as u32,
            &mut used as *mut u32 as u32,
        )
    };
    match errno {
        0 => {
            let n = (used as usize).min(buf.len());
            let text = String::from_utf8_lossy(&buf[..n]);
            // One TSV line: prettify same way list does.
            let line = text.lines().next().unwrap_or("");
            let mut it = line.split('\t');
            let n_ = it.next().unwrap_or("");
            let st = it.next().unwrap_or("");
            let pid = it.next().unwrap_or("-");
            let runs = it.next().unwrap_or("0");
            let path = it.next().unwrap_or("");
            println!("{} status={} pid={} runs={} path={}", n_, st, pid, runs, path);
        }
        1 => { eprintln!("service: {}: not found", name); std::process::exit(1); }
        n => { eprintln!("service: status: errno {}", n); std::process::exit(1); }
    }
}

fn usage_err(msg: &str) -> ! {
    eprintln!("service: {}", msg);
    eprintln!("usage: service [list|start <name>|status <name>|stop <name>]");
    std::process::exit(2);
}
