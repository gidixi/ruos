//! install — install ruos onto the first SATA disk (author + copy boot tree).
//!
//! DESTRUCTIVE: lays down a GPT + FAT32 ESP (with /EFI/BOOT) + FAT32 data
//! partition on the first populated SATA disk, wiping whatever was there, then
//! copies the full boot payload (kernel, BOOTX64.EFI, limine.conf + every
//! module) so the SSD boots standalone (UEFI → /EFI/BOOT/BOOTX64.EFI →
//! limine.conf → kernel). The kernel host fn `ruos::install` does all the work
//! AND guards it: it refuses when /mnt is mounted, so you cannot wipe the data
//! disk you booted from — boot the installer medium to install.
//!
//! Usage:
//!   install [esp_mib]      esp_mib defaults to 64

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn install(esp_mib: i32) -> i32;
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let esp_mib: i32 = match args.first() {
        Some(a) => match a.parse::<i32>() {
            Ok(n) if n > 0 => n,
            _ => {
                eprintln!("install: invalid ESP size '{}'", a);
                std::process::exit(1);
            }
        },
        None => 64,
    };

    eprintln!(
        "install: installing ruos to the first SATA disk (ESP {} MiB)...",
        esp_mib
    );

    let rc = unsafe { install(esp_mib) };
    match rc {
        0 => println!("install: ok — remove the installer medium and reboot"),
        -3 => {
            eprintln!("install: /mnt is mounted, refusing (boot the installer medium)");
            std::process::exit(1);
        }
        -1 => {
            eprintln!("install: no SATA disk");
            std::process::exit(1);
        }
        _ => {
            eprintln!("install: failed (code {})", rc);
            std::process::exit(1);
        }
    }
}
