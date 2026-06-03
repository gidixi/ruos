//! install -- install ruos onto a chosen SATA disk (author + copy boot tree).
//!
//! `install` with no argument LISTS the SATA disks (`[idx] model (N MiB)`) and
//! wipes nothing -- pick a disk on a multi-disk machine without risk.
//!
//! `install <idx> [esp_mib]` is DESTRUCTIVE: it lays down a GPT + FAT32 ESP
//! (with /EFI/BOOT) + FAT32 data partition on SATA disk `idx`, wiping whatever
//! was there, then copies the full boot payload (kernel, BOOTX64.EFI,
//! limine.conf + every module) so the SSD boots standalone (UEFI →
//! /EFI/BOOT/BOOTX64.EFI → limine.conf → kernel). The kernel host fn
//! `ruos::install` does all the work AND guards it: it refuses when /mnt is
//! mounted, so you cannot wipe the data disk you booted from -- boot the
//! installer medium to install.
//!
//! Usage:
//!   install                  list the SATA disks (wipes nothing)
//!   install <idx> [esp_mib]  install onto SATA disk <idx>; esp_mib defaults to 64

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn install(esp_mib: i32, target: i32) -> i32;
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // No disk argument → LIST mode. The kernel prints the `[idx] model` lines.
    if args.len() < 2 {
        eprintln!("install: SATA disks (run `install <n>` to install -- WIPES that disk):");
        let _ = unsafe { install(64, -1) };
        eprintln!("install: run `install <n>` to install onto disk <n>");
        return;
    }

    // install <n> [esp_mib]
    let target: i32 = match args[1].parse::<i32>() {
        Ok(n) if n >= 0 => n,
        _ => {
            eprintln!(
                "install: invalid disk index '{}' (run `install` with no argument to list)",
                args[1]
            );
            std::process::exit(1);
        }
    };
    let esp_mib: i32 = match args.get(2) {
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
        "install: installing ruos to SATA disk {} (ESP {} MiB)...",
        target, esp_mib
    );

    let rc = unsafe { install(esp_mib, target) };
    match rc {
        0 => println!("install: ok -- remove the installer medium and reboot"),
        -3 => {
            eprintln!("install: /mnt is mounted, refusing (boot the installer medium)");
            std::process::exit(1);
        }
        -11 => {
            eprintln!("install: no SATA disk at index {} (run `install` to list)", target);
            std::process::exit(1);
        }
        -1 => {
            eprintln!("install: no SATA disk / port not ready");
            std::process::exit(1);
        }
        -2 => {
            eprintln!("install: failed (write error -- disk left partially written)");
            std::process::exit(1);
        }
        _ => {
            eprintln!("install: failed (code {})", rc);
            std::process::exit(1);
        }
    }
}
