use std::io::Read;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = match args.get(1) {
        Some(p) => p.clone(),
        None => {
            eprintln!("cat: missing path");
            std::process::exit(1);
        }
    };
    match std::fs::File::open(&path) {
        Ok(mut f) => {
            let mut buf = Vec::new();
            if let Err(e) = f.read_to_end(&mut buf) {
                eprintln!("cat: {}: {}", path, e);
                std::process::exit(1);
            }
            if let Ok(s) = std::str::from_utf8(&buf) {
                print!("{}", s);
            } else {
                print!("(binary, {} bytes)", buf.len());
            }
        }
        Err(e) => {
            eprintln!("cat: {}: {}", path, e);
            std::process::exit(1);
        }
    }
}
