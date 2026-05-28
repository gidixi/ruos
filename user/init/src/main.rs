use std::fs::OpenOptions;
use std::io::Write;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn main() {
    println!("\x1b[1;32m╔══════════════════════════════════╗");
    println!("║         Welcome to ruos          ║");
    println!("║   wasm32-wasip1 / WASIX host     ║");
    println!("╚══════════════════════════════════╝\x1b[0m");

    // Step 10.5 T1: cooperative sleep proof.
    // std::thread::sleep compiles to __wasi_poll_oneoff with a clock subscription.
    thread::sleep(Duration::from_millis(500));
    println!("init.wasm: slept ok");

    // Step 10 Task 3 VFS smoke.
    if let Ok(mut f) = OpenOptions::new().write(true).create(true).open("/wasm-smoke.bin") {
        if f.write_all(b"0123456789").is_ok() {
            println!("init.wasm: vfs smoke ok");
        }
    }

    // Step 10 Task 4 clock + random.
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let ms = elapsed.as_millis();
    let mut rand_buf = [0u8; 16];
    getrandom::getrandom(&mut rand_buf).unwrap();
    print!("init.wasm: uptime_ms={} rand=", ms);
    for b in rand_buf { print!("{:02x}", b); }
    println!();
    println!("init.wasm: clock_rand ok");
}
