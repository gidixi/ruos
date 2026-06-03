//! mkboot -- author a fresh ruos disk AND write the full boot tree onto it.
//!
//! DESTRUCTIVE: lays down a GPT + FAT32 ESP (with /EFI/BOOT) + FAT32 data
//! partition on the first populated SATA disk, wiping whatever was there, then
//! copies the boot payload (kernel, BOOTX64.EFI, limine.conf + every module) so
//! the SSD boots standalone (UEFI → /EFI/BOOT/BOOTX64.EFI → limine.conf →
//! kernel). All the work happens in the kernel host fn `ruos::mkboot`; this tool
//! is a thin wrapper that parses the ESP size and prints the result.
//!
//! Usage:
//!   mkboot [esp_mib]      esp_mib defaults to 64

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn mkboot(esp_mib: i32) -> i32;
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let esp_mib: i32 = match args.first() {
        Some(a) => match a.parse::<i32>() {
            Ok(n) if n > 0 => n,
            _ => {
                eprintln!("mkboot: invalid ESP size '{}'", a);
                std::process::exit(1);
            }
        },
        None => 64,
    };

    eprintln!(
        "mkboot: authoring first SATA disk (ESP {} MiB) + writing boot tree -- wipes it",
        esp_mib
    );

    let rc = unsafe { mkboot(esp_mib) };
    if rc == 0 {
        println!("mkboot: ok");
    } else {
        eprintln!("mkboot: failed (code {})", rc);
        std::process::exit(1);
    }
}
