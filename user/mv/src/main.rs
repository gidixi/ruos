fn main() {
    ruos_rt::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        eprintln!("mv: usage: mv <src> <dst>");
        std::process::exit(1);
    }
    if let Err(e) = std::fs::rename(&args[0], &args[1]) {
        eprintln!("mv: {} -> {}: {}", args[0], args[1], e);
        std::process::exit(1);
    }
}
