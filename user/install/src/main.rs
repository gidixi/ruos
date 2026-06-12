//! install — guided ruos installer (Debian-style: list disks → confirm → progress).
//!
//! Modalità:
//!   install              guided interattivo: lista dischi, chiede target, conferma,
//!                        poi installa con barra di avanzamento.
//!   install <n>          batch non-interattivo: installa su disco <n> (ESP 64 MiB),
//!                        stampa riga per riga i file copiati; emette "install: ok"
//!                        alla fine (compatibile con m2b2-test.sh).
//!   install <n> <esp>    batch con ESP <esp> MiB personalizzato.

#[link(wasm_import_module = "ruos")]
extern "C" {
    /// Lista dischi SATA: "<idx>\t<model>\t<N> MiB\n" per riga → byte scritti,
    /// 0 se assente, -1 se buffer troppo piccolo.
    fn sata_list(ptr: *mut u8, cap: i32) -> i32;

    /// Apre sessione di installazione stepped. Ritorna total_files (>0) o codice
    /// errore negativo (-3 /mnt, -4 sessione già aperta, -11 no disco, -1/-2 err).
    fn install_open(target: i32, esp_mib: i32) -> i32;

    /// Copia il prossimo file della sessione. Scrive il nome in name_buf (C-string).
    /// Ritorna done_count (1..=total) oppure -5 (no sessione) / -2 (I/O error).
    fn install_step(name_buf: *mut u8, name_cap: i32) -> i32;
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    match args.len() {
        // ── Guided interactive ───────────────────────────────────────────────
        1 => guided(),
        // ── Batch: install <n> [esp_mib] ────────────────────────────────────
        2 | 3 => {
            let target = parse_index(&args[1]);
            let esp_mib = args.get(2).map(|a| parse_esp(a)).unwrap_or(64);
            batch(target, esp_mib);
        }
        _ => {
            eprintln!("usage: install [<disk_idx> [<esp_mib>]]");
            std::process::exit(1);
        }
    }
}

// ─── Guided mode ─────────────────────────────────────────────────────────────

fn guided() {
    println!();
    println!("  ruos installer");
    println!("  ──────────────────────────────────────────────────────────────");
    println!();

    // 1. Lista dischi
    let mut buf = [0u8; 2048];
    let n = unsafe { sata_list(buf.as_mut_ptr(), buf.len() as i32) };
    if n == 0 {
        eprintln!("  No SATA disks found.");
        std::process::exit(1);
    }
    if n < 0 {
        eprintln!("  Disk list too large.");
        std::process::exit(1);
    }
    let text = String::from_utf8_lossy(&buf[..n as usize]);
    let disks: Vec<DiskInfo> = text.lines().filter_map(parse_disk_line).collect();
    if disks.is_empty() {
        eprintln!("  No SATA disks found.");
        std::process::exit(1);
    }
    println!("  Available disks:");
    for d in &disks {
        println!("    #{:<3}  {:<36}  {} MiB", d.idx, d.model, d.mib);
    }
    println!();

    // 2. Scelta disco
    let target = prompt_int("  Target disk number", disks[0].idx as i32);
    // Verifica che il disco scelto esista
    if !disks.iter().any(|d| d.idx == target as usize) {
        eprintln!("  install: no disk at index {}", target);
        std::process::exit(1);
    }
    let chosen = disks.iter().find(|d| d.idx == target as usize).unwrap();

    // 3. Dimensione ESP
    let esp_mib = prompt_int("  ESP partition size [MiB]", 64);
    if esp_mib <= 0 || esp_mib > 4096 {
        eprintln!("  install: ESP size must be 1-4096 MiB");
        std::process::exit(1);
    }

    // 4. Conferma — sicurezza esplicita
    println!();
    println!("  !! WARNING: disk #{} ({}, {} MiB) will be ERASED.", chosen.idx, chosen.model, chosen.mib);
    println!("  !! ALL DATA ON THAT DISK WILL BE LOST.");
    println!();
    let confirm = prompt_str("  Type YES to confirm");
    if confirm.trim() != "YES" {
        println!("  Aborted.");
        return;
    }
    println!();

    // 5. Installazione con barra di avanzamento
    run_install(target, esp_mib, true);
}

// ─── Batch mode ──────────────────────────────────────────────────────────────

fn batch(target: i32, esp_mib: i32) {
    eprintln!("install: installing ruos to disk #{} (ESP {} MiB)...", target, esp_mib);
    run_install(target, esp_mib, false);
}

// ─── Common install runner ────────────────────────────────────────────────────

fn run_install(target: i32, esp_mib: i32, interactive: bool) {
    // Apri sessione
    let total = unsafe { install_open(target, esp_mib) };
    match total {
        n if n > 0 => {} // ok, n = total files
        -3 => {
            eprintln!("install: /mnt is mounted, refusing (boot the installer medium)");
            std::process::exit(1);
        }
        -11 => {
            eprintln!("install: no SATA disk at index {} (run `disks` to list)", target);
            std::process::exit(1);
        }
        -4 => {
            eprintln!("install: another install session is already running");
            std::process::exit(1);
        }
        -1 => {
            eprintln!("install: port not ready");
            std::process::exit(1);
        }
        _ => {
            eprintln!("install: failed to prepare disk (code {})", total);
            std::process::exit(1);
        }
    }
    let total = total as usize;

    if interactive {
        eprintln!("  Installing {} files:", total);
    }

    let mut name_buf = [0u8; 256];
    let mut done = 0usize;
    loop {
        let rc = unsafe { install_step(name_buf.as_mut_ptr(), name_buf.len() as i32) };
        if rc < 0 {
            let name = cstr_to_str(&name_buf);
            if interactive {
                eprintln!(); // newline after progress bar
            }
            eprintln!("install: failed while copying '{}' (code {})", name, rc);
            std::process::exit(1);
        }
        done = rc as usize;
        let name = cstr_to_str(&name_buf);

        if interactive {
            print_progress(done, total, name);
        } else {
            eprintln!("  [{:>3}/{}] {}", done, total, name);
        }

        if done == total {
            break;
        }
    }

    if interactive {
        eprintln!(); // newline dopo l'ultima riga progress
        eprintln!();
        eprintln!("  Install complete! Remove the installer medium and reboot.");
        eprintln!();
    } else {
        println!("install: ok -- remove the installer medium and reboot");
    }
}

// ─── Progress bar (interactive) ──────────────────────────────────────────────

fn print_progress(done: usize, total: usize, name: &str) {
    const BAR: usize = 40;
    let filled = if total > 0 { done * BAR / total } else { BAR };
    let pct = if total > 0 { done * 100 / total } else { 100 };

    // Tronca nome a 28 caratteri per stare in 80 colonne
    let name_disp = if name.len() > 28 { &name[..28] } else { name };

    // \r sovrascrive la riga corrente (progress in-place)
    eprint!("\r  [");
    for i in 0..BAR {
        if i < filled { eprint!("\u{2588}"); } else { eprint!("░"); }
    }
    eprint!("] {:>3}%  {:<28}", pct, name_disp);
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

struct DiskInfo {
    idx:   usize,
    model: String,
    mib:   u64,
}

fn parse_disk_line(line: &str) -> Option<DiskInfo> {
    let mut f = line.split('\t');
    let idx   = f.next()?.trim().parse::<usize>().ok()?;
    let model = f.next()?.trim_matches('"').to_string();
    let mib_s = f.next()?.split_whitespace().next()?;
    let mib   = mib_s.parse::<u64>().ok()?;
    Some(DiskInfo { idx, model, mib })
}

fn prompt_int(prompt: &str, default: i32) -> i32 {
    use std::io::{Write, BufRead};
    eprint!("{} [{}]: ", prompt, default);
    std::io::stderr().flush().ok();
    let line = std::io::stdin().lock().lines().next()
        .and_then(|r| r.ok())
        .unwrap_or_default();
    let trimmed = line.trim();
    if trimmed.is_empty() { default } else { trimmed.parse::<i32>().unwrap_or(default) }
}

fn prompt_str(prompt: &str) -> String {
    use std::io::{Write, BufRead};
    eprint!("{}: ", prompt);
    std::io::stderr().flush().ok();
    std::io::stdin().lock().lines().next()
        .and_then(|r| r.ok())
        .unwrap_or_default()
}

fn parse_index(s: &str) -> i32 {
    match s.parse::<i32>() {
        Ok(n) if n >= 0 => n,
        _ => { eprintln!("install: invalid disk index '{}'", s); std::process::exit(1); }
    }
}

fn parse_esp(s: &str) -> i32 {
    match s.parse::<i32>() {
        Ok(n) if n > 0 => n,
        _ => { eprintln!("install: invalid ESP size '{}'", s); std::process::exit(1); }
    }
}

fn cstr_to_str(buf: &[u8]) -> &str {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    core::str::from_utf8(&buf[..end]).unwrap_or("?")
}

