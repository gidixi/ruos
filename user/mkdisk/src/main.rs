//! mkdisk — author a fresh ruos disk on the first SATA port.
//!
//! DESTRUCTIVE: lays down a GPT + FAT32 ESP (with /EFI/BOOT) + FAT32 data
//! partition on the first populated SATA disk, wiping whatever was there.
//! All the work happens in the kernel host fn `ruos::mkdisk`; this tool is a
//! thin wrapper that parses the ESP size and prints the result.
//!
//! Usage:
//!   mkdisk [esp_mib]      esp_mib defaults to 64

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn mkdisk(esp_mib: i32) -> i32;
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let esp_mib: i32 = match args.first() {
        Some(a) => match a.parse::<i32>() {
            Ok(n) if n > 0 => n,
            _ => {
                eprintln!("mkdisk: invalid ESP size '{}'", a);
                std::process::exit(1);
            }
        },
        None => 64,
    };

    eprintln!(
        "mkdisk: authoring first SATA disk (ESP {} MiB) — wipes it",
        esp_mib
    );

    let rc = unsafe { mkdisk(esp_mib) };
    if rc == 0 {
        println!("mkdisk: ok");
    } else {
        eprintln!("mkdisk: failed (code {})", rc);
        std::process::exit(1);
    }
}
