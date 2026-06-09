fn main() {
    ruos_rt::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("rmdir: missing operand");
        std::process::exit(1);
    }
    let mut code = 0;
    for p in &args {
        if let Err(e) = std::fs::remove_dir(p) {
            eprintln!("rmdir: {}: {}", p, e);
            code = 1;
        }
    }
    std::process::exit(code);
}
