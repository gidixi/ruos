use std::fs::OpenOptions;
use std::io::{Read, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let append = args.iter().any(|a| a == "-a");
    let files: Vec<String> = args.iter().filter(|a| !a.starts_with('-')).cloned().collect();

    let mut data = Vec::<u8>::new();
    let _ = std::io::stdin().read_to_end(&mut data);

    // Write to stdout.
    let _ = std::io::stdout().write_all(&data);

    for f in &files {
        let r = OpenOptions::new()
            .create(true).write(true)
            .truncate(!append).append(append)
            .open(f);
        match r {
            Ok(mut fh) => { let _ = fh.write_all(&data); }
            Err(e) => eprintln!("tee: {}: {}", f, e),
        }
    }
}
