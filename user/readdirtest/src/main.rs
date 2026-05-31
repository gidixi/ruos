//! WASI fd_readdir smoke: enumerate a directory via the *standard* library
//! path (`std::fs::read_dir`), which lowers to `fd_readdir` on wasm32-wasip1
//! — NOT the custom `ruos.readdir` host fn. Proves the WASI directory ABI.
//!
//! Prints a single marker line `readdir-std: <N> entries` for the boot smoke
//! to assert on. `.`/`..` are already filtered out by std::fs::read_dir.

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "/bin".to_string());
    match std::fs::read_dir(&dir) {
        Ok(entries) => {
            let mut count = 0usize;
            for e in entries {
                if e.is_ok() {
                    count += 1;
                }
            }
            println!("readdir-std: {} entries", count);
        }
        Err(e) => {
            println!("readdir-std: ERROR {}", e);
            std::process::exit(1);
        }
    }
}
