fn main() {
    ruos_rt::init();
    let args: Vec<String> = std::env::args().collect();
    let mut paths: Vec<String> = Vec::new();
    let mut parents = false;
    for a in args.iter().skip(1) {
        if a == "-p" || a == "--parents" {
            parents = true;
        } else if a.starts_with('-') {
            eprintln!("mkdir: unknown option: {}", a);
            std::process::exit(1);
        } else {
            paths.push(a.clone());
        }
    }
    if paths.is_empty() {
        eprintln!("mkdir: missing operand");
        std::process::exit(1);
    }
    let mut code = 0;
    for p in &paths {
        let r = if parents { std::fs::create_dir_all(p) } else { std::fs::create_dir(p) };
        if let Err(e) = r {
            eprintln!("mkdir: {}: {}", p, e);
            code = 1;
        }
    }
    std::process::exit(code);
}
