#[link(wasm_import_module = "ruos")]
extern "C" {
    fn proc_kill(pid: i32) -> i32;
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("kill: usage: kill <pid> [pid ...]");
        std::process::exit(1);
    }
    let mut code = 0;
    for a in &args {
        // tolerate "-9" signals — we ignore them, only one kill semantic.
        let s = a.trim_start_matches('-');
        let pid: i32 = match s.parse() {
            Ok(n) => n,
            Err(_) => { eprintln!("kill: invalid pid: {}", a); code = 1; continue; }
        };
        let errno = unsafe { proc_kill(pid) };
        if errno != 0 {
            eprintln!("kill: {}: no such process", pid);
            code = 1;
        }
    }
    std::process::exit(code);
}
