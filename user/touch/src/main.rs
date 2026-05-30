use std::fs::OpenOptions;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: touch <file>...");
        std::process::exit(2);
    }
    let mut ec = 0;
    for f in &args {
        match OpenOptions::new().create(true).write(true).open(f) {
            Ok(_)  => {}
            Err(e) => { eprintln!("touch: {}: {}", f, e); ec = 1; }
        }
    }
    std::process::exit(ec);
}
