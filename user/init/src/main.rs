use std::fs::OpenOptions;
use std::io::Write;

fn main() {
    println!("\x1b[1;32m╔══════════════════════════════════╗");
    println!("║         Welcome to ruos          ║");
    println!("║   wasm32-wasip1 / WASIX host     ║");
    println!("╚══════════════════════════════════╝\x1b[0m");

    match OpenOptions::new().write(true).create(true).open("/wasm-smoke.bin") {
        Ok(mut f) => match f.write_all(b"0123456789") {
            Ok(()) => println!("init.wasm: vfs smoke ok"),
            Err(e) => println!("init.wasm: vfs write fail: {}", e),
        },
        Err(e) => println!("init.wasm: vfs open fail: {}", e),
    }
}
