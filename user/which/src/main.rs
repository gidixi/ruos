use std::fs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: which <cmd>...");
        std::process::exit(2);
    }
    let mut ec = 0;
    for a in &args {
        let mut found = false;
        for prefix in ["/bin/", "/usr/bin/"] {
            let path = alloc::format!("{}{}.wasm", prefix, a);
            if fs::metadata(&path).is_ok() {
                println!("{}", path);
                found = true;
                break;
            }
        }
        if !found {
            eprintln!("which: {}: not found", a);
            ec = 1;
        }
    }
    std::process::exit(ec);
}
extern crate alloc;
