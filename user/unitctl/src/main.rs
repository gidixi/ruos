//! `unitctl` — init system CLI (estende `service`). Host fns `ruos.unit_*`.
//!
//! unitctl [list] | status <n> | start <n> | stop <n> | enable <n> |
//!         disable <n> | timers | reload | cat <n>
//!
//! Exit: 0 ok; 1 errore (errno != 0); 2 usage.

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn unit_list(buf_ptr: u32, buf_len: u32, used_ptr: u32) -> i32;
    fn unit_status(name_ptr: u32, name_len: u32, buf_ptr: u32, buf_len: u32, used_ptr: u32) -> i32;
    fn unit_start(name_ptr: u32, name_len: u32) -> i32;
    fn unit_stop(name_ptr: u32, name_len: u32) -> i32;
    fn unit_enable(name_ptr: u32, name_len: u32, on: i32) -> i32;
    fn timer_list(buf_ptr: u32, buf_len: u32, used_ptr: u32) -> i32;
    fn unit_reload() -> i32;
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let sub = args.first().map(String::as_str).unwrap_or("list");
    match sub {
        "list"    => cmd_list(),
        "status"  => cmd_status(name_arg(&args)),
        "start"   => cmd_errno("start",   call1(name_arg(&args), |p, l| unsafe { unit_start(p, l) })),
        "stop"    => cmd_errno("stop",    call1(name_arg(&args), |p, l| unsafe { unit_stop(p, l) })),
        "enable"  => cmd_errno("enable",  call1(name_arg(&args), |p, l| unsafe { unit_enable(p, l, 1) })),
        "disable" => cmd_errno("disable", call1(name_arg(&args), |p, l| unsafe { unit_enable(p, l, 0) })),
        "timers"  => cmd_timers(),
        "reload"  => cmd_errno("reload",  unsafe { unit_reload() }),
        "cat"     => cmd_cat(name_arg(&args)),
        other => usage_err(&format!("unknown subcommand: {}", other)),
    }
}

fn name_arg(args: &[String]) -> &str {
    match args.get(1) { Some(n) => n, None => usage_err("missing unit name") }
}

fn call1(name: &str, f: impl Fn(u32, u32) -> i32) -> i32 {
    let b = name.as_bytes();
    f(b.as_ptr() as u32, b.len() as u32)
}

fn fetch(f: impl Fn(u32, u32, u32) -> i32) -> Option<String> {
    let mut buf = vec![0u8; 16384];
    let mut used: u32 = 0;
    let errno = f(buf.as_mut_ptr() as u32, buf.len() as u32, &mut used as *mut u32 as u32);
    if errno != 0 { eprintln!("unitctl: errno {}", errno); return None; }
    let n = (used as usize).min(buf.len());
    Some(String::from_utf8_lossy(&buf[..n]).into_owned())
}

fn cmd_list() {
    let Some(text) = fetch(|b, l, u| unsafe { unit_list(b, l, u) }) else { std::process::exit(1) };
    println!("{:<14} {:<8} {:<14} {:>5} {:>4} {:>4}  {:<9} {:<3}  {}",
        "NAME", "KIND", "STATUS", "PID", "RUNS", "RST", "TARGET", "EN", "PATH");
    for line in text.lines().filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 10 { continue; }
        let en = if f[7] == "true" { "on" } else { "off" };
        println!("{:<14} {:<8} {:<14} {:>5} {:>4} {:>4}  {:<9} {:<3}  {}",
            f[0], f[1], f[2], f[3], f[4], f[5], f[6], en, f[8]);
    }
}

fn cmd_status(name: &str) {
    let b = name.as_bytes();
    let Some(text) = fetch(|buf, l, u| unsafe {
        unit_status(b.as_ptr() as u32, b.len() as u32, buf, l, u)
    }) else { std::process::exit(1) };
    let f: Vec<&str> = text.trim_end().split('\t').collect();
    if f.len() < 10 { eprintln!("unitctl: bad record"); std::process::exit(1); }
    println!("{} kind={} status={} pid={} runs={} restarts={} target={} enabled={}",
        f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7]);
    println!("  exec: {}", f[8]);
    if f[9] != "-" { println!("  file: {}", f[9]); }
}

fn cmd_timers() {
    let Some(text) = fetch(|b, l, u| unsafe { timer_list(b, l, u) }) else { std::process::exit(1) };
    println!("{:<14} {:<14} {:<18} {:<3}  {:>12} {:>12}",
        "NAME", "UNIT", "SCHEDULE", "EN", "NEXT", "LAST");
    for line in text.lines().filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 6 { continue; }
        let en = if f[3] == "true" { "on" } else { "off" };
        println!("{:<14} {:<14} {:<18} {:<3}  {:>12} {:>12}", f[0], f[1], f[2], en, f[4], f[5]);
    }
}

fn cmd_errno(op: &str, errno: i32) {
    match errno {
        0 => println!("unitctl: {}: ok", op),
        1 => { eprintln!("unitctl: {}: not found", op);           std::process::exit(1); }
        2 => { eprintln!("unitctl: {}: already running", op);     std::process::exit(1); }
        3 => { eprintln!("unitctl: {}: not supported", op);       std::process::exit(1); }
        4 => { eprintln!("unitctl: {}: no free daemon slot", op); std::process::exit(1); }
        n => { eprintln!("unitctl: {}: errno {}", op, n);         std::process::exit(1); }
    }
}

fn cmd_cat(name: &str) {
    // colonna 10 (file) dello status → lettura via WASI
    let b = name.as_bytes();
    let Some(text) = fetch(|buf, l, u| unsafe {
        unit_status(b.as_ptr() as u32, b.len() as u32, buf, l, u)
    }) else { std::process::exit(1) };
    let file = text.trim_end().split('\t').nth(9).unwrap_or("-");
    if file == "-" { eprintln!("unitctl: cat: '{}' has no source file", name); std::process::exit(1); }
    match std::fs::read_to_string(file) {
        Ok(s) => print!("{}", s),
        Err(e) => { eprintln!("unitctl: cat {}: {}", file, e); std::process::exit(1); }
    }
}

fn usage_err(msg: &str) -> ! {
    eprintln!("unitctl: {}", msg);
    eprintln!("usage: unitctl [list|status <n>|start <n>|stop <n>|enable <n>|disable <n>|timers|reload|cat <n>]");
    std::process::exit(2);
}
