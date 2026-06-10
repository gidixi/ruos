//! mkbinpack OUT IN...  — impacchetta i file IN in un container RBIN (OUT),
//! usando il nome-file (basename) come nome entry. Tool host del build ruos.

use std::path::Path;
use std::process::exit;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: mkbinpack OUT IN...");
        exit(2);
    }
    let out = &args[1];
    let inputs = &args[2..];

    let datas: Vec<(String, Vec<u8>)> = inputs
        .iter()
        .map(|p| {
            let name = Path::new(p)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| {
                    eprintln!("mkbinpack: bad path {}", p);
                    exit(1);
                });
            let bytes = std::fs::read(p).unwrap_or_else(|e| {
                eprintln!("mkbinpack: {}: {}", p, e);
                exit(1);
            });
            (name, bytes)
        })
        .collect();

    let refs: Vec<(&str, &[u8])> =
        datas.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let archive = gzip_core::pack::write_archive(&refs, 6);

    std::fs::write(out, &archive).unwrap_or_else(|e| {
        eprintln!("mkbinpack: {}: {}", out, e);
        exit(1);
    });
    eprintln!(
        "mkbinpack: {} entries, {} bytes -> {}",
        refs.len(),
        archive.len(),
        out
    );
}
