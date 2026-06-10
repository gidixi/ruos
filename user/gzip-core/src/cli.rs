//! CLI condivisa da gzip/gunzip/zcat. Parsing flag + dispatch file/stdin,
//! semantica Unix classica (vedi spec 2026-06-10-gzip-tools-design).

use std::io::{Read, Write};
use std::path::Path;
use std::process::exit;

use crate::format::{compress, decompress};

/// Entrypoint condiviso dai tre bin.
/// - `default_decompress`: decomprimi invece di comprimere (gunzip/zcat).
/// - `default_stdout`: scrivi su stdout e tieni l'input (zcat = `-dc`).
pub fn run_cli(default_decompress: bool, default_stdout: bool) -> ! {
    let mut argv = std::env::args();
    let prog = argv.next().map(|a| basename(&a)).unwrap_or_else(|| "gzip".into());

    let mut decompress_mode = default_decompress;
    let mut to_stdout = default_stdout;
    let mut keep = default_stdout; // zcat non cancella l'input
    let mut level = 6u8;
    let mut files: Vec<String> = Vec::new();

    for arg in argv {
        if arg.starts_with('-') && arg != "-" {
            for ch in arg[1..].chars() {
                match ch {
                    'c' => to_stdout = true,
                    'k' => keep = true,
                    'd' => decompress_mode = true,
                    'h' => {
                        usage(&prog);
                        exit(0);
                    }
                    '1'..='9' => level = ch as u8 - b'0',
                    _ => {
                        eprintln!("{}: invalid option -- '{}'", prog, ch);
                        usage(&prog);
                        exit(1);
                    }
                }
            }
        } else {
            files.push(arg);
        }
    }

    // Senza file: stdin→stdout (sempre, `-c` implicito).
    if files.is_empty() {
        exit(if process_stdin(&prog, decompress_mode, level) { 0 } else { 1 });
    }

    let mut failed = false;
    for f in &files {
        if !process_file(&prog, f, decompress_mode, to_stdout, keep, level) {
            failed = true;
        }
    }
    exit(if failed { 1 } else { 0 });
}

fn process_stdin(prog: &str, decompress_mode: bool, level: u8) -> bool {
    let mut data = Vec::new();
    if let Err(e) = std::io::stdin().read_to_end(&mut data) {
        eprintln!("{}: stdin: {}", prog, e);
        return false;
    }
    let out = if decompress_mode {
        match decompress(&data) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("{}: stdin: {}", prog, e);
                return false;
            }
        }
    } else {
        compress(&data, level)
    };
    write_stdout(prog, &out)
}

fn process_file(
    prog: &str,
    path: &str,
    decompress_mode: bool,
    to_stdout: bool,
    keep: bool,
    level: u8,
) -> bool {
    // Validazione suffisso prima di leggere (skip rapido).
    if !to_stdout {
        if decompress_mode {
            if !path.ends_with(".gz") {
                eprintln!("{}: {}: unknown suffix -- ignored", prog, path);
                return false;
            }
        } else if path.ends_with(".gz") {
            eprintln!("{}: {} already has .gz suffix -- unchanged", prog, path);
            return false;
        }
    }

    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{}: {}: {}", prog, path, e);
            return false;
        }
    };

    let out = if decompress_mode {
        match decompress(&data) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("{}: {}: {}", prog, path, e);
                return false;
            }
        }
    } else {
        compress(&data, level)
    };

    if to_stdout {
        return write_stdout(prog, &out);
    }

    let outname = if decompress_mode {
        path[..path.len() - 3].to_string() // togli ".gz"
    } else {
        format!("{}.gz", path)
    };

    if Path::new(&outname).exists() {
        eprintln!("{}: {} already exists -- not overwritten", prog, outname);
        return false;
    }
    if let Err(e) = std::fs::write(&outname, &out) {
        eprintln!("{}: {}: {}", prog, outname, e);
        return false;
    }
    if !keep {
        if let Err(e) = std::fs::remove_file(path) {
            eprintln!("{}: {}: {}", prog, path, e);
            return false;
        }
    }
    true
}

fn write_stdout(prog: &str, data: &[u8]) -> bool {
    let mut out = std::io::stdout();
    if let Err(e) = out.write_all(data).and_then(|_| out.flush()) {
        eprintln!("{}: stdout: {}", prog, e);
        return false;
    }
    true
}

fn usage(prog: &str) {
    eprintln!("usage: {} [-c -k -d -1..-9 -h] [FILE...]", prog);
    eprintln!("  -c  write to stdout, keep files     -k  keep input files");
    eprintln!("  -d  decompress                      -1..-9  compression level");
}

/// Nome programma per i messaggi d'errore: basename senza estensione `.wasm`.
fn basename(arg0: &str) -> String {
    let base = arg0.rsplit(['/', '\\']).next().unwrap_or(arg0);
    base.strip_suffix(".wasm").unwrap_or(base).to_string()
}
