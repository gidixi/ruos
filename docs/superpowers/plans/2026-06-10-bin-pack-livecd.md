# Live-CD /bin via bin.bgz Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Popolare `/bin` decomprimendo un singolo archivio `bin.bgz` caricato da Limine come modulo in RAM, eliminando la dipendenza runtime da USB-MSC/ATAPI e rendendo la chiavetta USB scollegabile.

**Architecture:** `mkbinpack` (host) impacchetta tutti i bin in un container `RBIN` (un membro gzip indipendente per file). Limine carica `bin.bgz` come modulo opaco in HHDM. Una nuova fase boot `unpack_bin` (subito dopo `fs`) lo parsa, decomprime ogni membro con `gzip-core` (reso no_std) e scrive i file in tmpfs `/bin`. Un set rescue minimo resta come moduli `/rescue/` per il fallback.

**Tech Stack:** Rust no_std (kernel), Rust std (mkbinpack host), `miniz_oxide` (inflate/deflate), Limine boot modules, Makefile, QEMU smoke test.

---

## File Structure

- `user/gzip-core/` — reso `no_std` + feature `std`; nuovo modulo `pack` (formato container, no_std, condiviso kernel+host).
- `tools/mkbinpack/` — nuovo tool host std: impacchetta file → `bin.bgz`.
- `kernel/src/boot/phases/unpack_bin.rs` — nuova fase: archivio → tmpfs `/bin` (+ rescue fallback).
- `kernel/src/modules.rs` — skip prefissi `/archive/` e `/rescue/`; accessor `archive()` + `rescue_all()`.
- `kernel/src/boot/{mod.rs,phases/mod.rs}` — rimpiazza `media_bin` con `unpack_bin`.
- `kernel/src/executor/mod.rs` — rimuove `bin_overlay_task` (spawn + def).
- `kernel/src/boot/phases/media_bin.rs` — eliminato.
- `kernel/Cargo.toml` — dip. `gzip-core` (no default features).
- `limine.conf` — rimuove i /bin loose; aggiunge `bin.bgz` (`/archive/`) + rescue set (`/rescue/`).
- `Makefile` — build `mkbinpack`, pack `bin.bgz`, ISO ship solo `bin.bgz` + `/rescue/`; aggiorna asserzioni `run-test`/`run-test-usb`.

Tutti i comandi build/test girano in WSL:
`wsl -d Ubuntu-22.04 -u root -e bash -lc 'source ~/.cargo/env && cd /mnt/w/Work/GitHub/ruos && <cmd>'`

---

## Task 1: gzip-core → no_std + feature std

**Files:**
- Modify: `user/gzip-core/Cargo.toml`
- Modify: `user/gzip-core/src/lib.rs`
- Modify: `user/gzip-core/src/format.rs:1-7` (add alloc imports)

- [ ] **Step 1: Verifica che i test attuali passino (baseline)**

Run: `wsl -d Ubuntu-22.04 -u root -e bash -lc 'source ~/.cargo/env && cd /mnt/w/Work/GitHub/ruos/user && cargo test -p gzip-core 2>&1 | tail -5'`
Expected: `test result: ok. 18 passed`

- [ ] **Step 2: Cargo.toml — miniz no_std + feature std**

Sostituisci la sezione `[dependencies]` e aggiungi `[features]`:

```toml
[dependencies]
miniz_oxide = { version = "0.8", default-features = false, features = ["with-alloc"] }

[features]
default = ["std"]
std = []

[lib]
name = "gzip_core"
path = "src/lib.rs"
```

- [ ] **Step 3: lib.rs — no_std condizionale + modulo pack + cli gated**

Sostituisci tutto `user/gzip-core/src/lib.rs` con:

```rust
//! gzip-core — formato gzip (RFC 1952) + deflate via miniz_oxide.
//! Logica condivisa dai tool gzip/gunzip/zcat e dal kernel (unpack bin.bgz).
//! `no_std` di default sotto kernel; feature `std` abilita la CLI userland.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod crc32;
mod format;
pub mod pack;

#[cfg(feature = "std")]
mod cli;
#[cfg(feature = "std")]
pub use cli::run_cli;

pub use format::{compress, decompress, GzError};
```

- [ ] **Step 4: format.rs — import alloc espliciti**

In testa a `user/gzip-core/src/format.rs`, dopo `use core::fmt;` (riga 4), aggiungi:

```rust
use alloc::boxed::Box;
use alloc::vec::Vec;
```

- [ ] **Step 5: Build-check no_std (target wasm = ambiente no_std)**

Run: `wsl -d Ubuntu-22.04 -u root -e bash -lc 'source ~/.cargo/env && cd /mnt/w/Work/GitHub/ruos/user && cargo build -p gzip-core --no-default-features --target wasm32-unknown-unknown 2>&1 | tail -15'`
Expected: `Finished` (compila senza std). Se errori `Vec/Box not found` → import mancanti in format.rs/pack.rs.

- [ ] **Step 6: Host test ancora verdi (feature std default)**

Run: `wsl -d Ubuntu-22.04 -u root -e bash -lc 'source ~/.cargo/env && cd /mnt/w/Work/GitHub/ruos/user && cargo test -p gzip-core 2>&1 | tail -5'`
Expected: `test result: ok. 18 passed` (più i test di pack dopo Task 2).

- [ ] **Step 7: Commit**

```bash
git add user/gzip-core/Cargo.toml user/gzip-core/src/lib.rs user/gzip-core/src/format.rs
git commit -m "refactor(gzip-core): no_std + feature std (riuso kernel)"
```

---

## Task 2: modulo pack — formato container RBIN

**Files:**
- Create: `user/gzip-core/src/pack.rs`

- [ ] **Step 1: Scrivi i test (roundtrip + parse error)**

Crea `user/gzip-core/src/pack.rs` con SOLO i test in coda (l'impl arriva allo Step 3). Per ora scrivi il file completo dello Step 3 ma parti verificando il fallimento di compilazione assente. Test da includere:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn roundtrip_two_files() {
        let files: &[(&str, &[u8])] = &[("ls.wasm", b"hello"), ("cat.wasm", &[0xAA; 5000])];
        let archive = write_archive(files, 6);
        let mut got: Vec<(alloc::string::String, Vec<u8>)> = Vec::new();
        for e in parse(&archive).unwrap() {
            let (name, gz) = e.unwrap();
            got.push((name.into(), decompress_member(gz).unwrap()));
        }
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0, "ls.wasm");
        assert_eq!(got[0].1, b"hello");
        assert_eq!(got[1].0, "cat.wasm");
        assert_eq!(got[1].1, vec![0xAA; 5000]);
    }

    #[test]
    fn empty_archive() {
        let archive = write_archive(&[], 6);
        assert_eq!(parse(&archive).unwrap().count(), 0);
    }

    #[test]
    fn rejects_bad_magic() {
        assert_eq!(parse(b"XXXX\x01\x00\x00\x00\x00").unwrap_err(), PackError::BadMagic);
    }

    #[test]
    fn rejects_truncated_header() {
        assert_eq!(parse(b"RB").unwrap_err(), PackError::Truncated);
    }

    #[test]
    fn rejects_truncated_entry() {
        // count=1 ma niente entry → l'iteratore segnala Truncated
        let mut a = Vec::new();
        a.extend_from_slice(b"RBIN");
        a.push(1u8);
        a.extend_from_slice(&1u32.to_le_bytes());
        let err = parse(&a).unwrap().next().unwrap().unwrap_err();
        assert_eq!(err, PackError::Truncated);
    }
}
```

- [ ] **Step 2: Run test — deve fallire (impl assente)**

Run: `wsl -d Ubuntu-22.04 -u root -e bash -lc 'source ~/.cargo/env && cd /mnt/w/Work/GitHub/ruos/user && cargo test -p gzip-core pack 2>&1 | tail -15'`
Expected: errore di compilazione (`cannot find function write_archive`).

- [ ] **Step 3: Implementa il modulo pack**

In testa a `user/gzip-core/src/pack.rs` (prima dei test), aggiungi:

```rust
//! Container "RBIN": più file, ognuno un membro gzip indipendente.
//!
//! Layout (little-endian):
//!   magic "RBIN" (4) | ver u8=1 | count u32
//!   per entry: name_len u16 | name | gz_len u32 | gzip-member
//!
//! Membri separati (non un singolo gzip di un tar) così il kernel decomprime
//! un file alla volta direttamente in tmpfs → peak heap basso.

use alloc::vec::Vec;

use crate::format::{compress, decompress, GzError};

const MAGIC: &[u8; 4] = b"RBIN";
const VERSION: u8 = 1;

#[derive(Debug, PartialEq, Eq)]
pub enum PackError {
    BadMagic,
    BadVersion,
    Truncated,
}

/// Costruisci l'archivio: ogni `(name, data)` → membro gzip. `level` 1..=9.
pub fn write_archive(entries: &[(&str, &[u8])], level: u8) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (name, data) in entries {
        let nb = name.as_bytes();
        out.extend_from_slice(&(nb.len() as u16).to_le_bytes());
        out.extend_from_slice(nb);
        let gz = compress(data, level);
        out.extend_from_slice(&(gz.len() as u32).to_le_bytes());
        out.extend_from_slice(&gz);
    }
    out
}

/// Iteratore sulle entry: `(name, gz_member)` senza decomprimere.
pub struct ArchiveIter<'a> {
    data: &'a [u8],
    pos: usize,
    remaining: u32,
}

/// Parsa l'header e ritorna l'iteratore sulle entry.
pub fn parse(data: &[u8]) -> Result<ArchiveIter<'_>, PackError> {
    if data.len() < 9 {
        return Err(PackError::Truncated);
    }
    if &data[..4] != MAGIC {
        return Err(PackError::BadMagic);
    }
    if data[4] != VERSION {
        return Err(PackError::BadVersion);
    }
    let count = u32::from_le_bytes(data[5..9].try_into().unwrap());
    Ok(ArchiveIter { data, pos: 9, remaining: count })
}

impl<'a> Iterator for ArchiveIter<'a> {
    type Item = Result<(&'a str, &'a [u8]), PackError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        let d = self.data;
        let mut p = self.pos;
        if p + 2 > d.len() {
            return Some(Err(PackError::Truncated));
        }
        let nlen = u16::from_le_bytes([d[p], d[p + 1]]) as usize;
        p += 2;
        if p + nlen + 4 > d.len() {
            return Some(Err(PackError::Truncated));
        }
        let name = match core::str::from_utf8(&d[p..p + nlen]) {
            Ok(s) => s,
            Err(_) => return Some(Err(PackError::Truncated)),
        };
        p += nlen;
        let glen = u32::from_le_bytes(d[p..p + 4].try_into().unwrap()) as usize;
        p += 4;
        if p + glen > d.len() {
            return Some(Err(PackError::Truncated));
        }
        let gz = &d[p..p + glen];
        p += glen;
        self.pos = p;
        Some(Ok((name, gz)))
    }
}

/// Decomprimi un membro (riusa `format::decompress`).
pub fn decompress_member(gz: &[u8]) -> Result<Vec<u8>, GzError> {
    decompress(gz)
}
```

- [ ] **Step 4: Run test — devono passare**

Run: `wsl -d Ubuntu-22.04 -u root -e bash -lc 'source ~/.cargo/env && cd /mnt/w/Work/GitHub/ruos/user && cargo test -p gzip-core pack 2>&1 | tail -10'`
Expected: `test result: ok. 5 passed` per i test di pack.

- [ ] **Step 5: Build-check no_std (pack compila senza std)**

Run: `wsl -d Ubuntu-22.04 -u root -e bash -lc 'source ~/.cargo/env && cd /mnt/w/Work/GitHub/ruos/user && cargo build -p gzip-core --no-default-features --target wasm32-unknown-unknown 2>&1 | tail -5'`
Expected: `Finished`.

- [ ] **Step 6: Commit**

```bash
git add user/gzip-core/src/pack.rs
git commit -m "feat(gzip-core): pack — container RBIN di membri gzip"
```

---

## Task 3: mkbinpack — tool host

**Files:**
- Create: `tools/mkbinpack/Cargo.toml`
- Create: `tools/mkbinpack/src/main.rs`

- [ ] **Step 1: Cargo.toml**

Crea `tools/mkbinpack/Cargo.toml`:

```toml
[package]
name = "mkbinpack"
version = "0.1.0"
edition = "2021"

[dependencies]
gzip-core = { path = "../../user/gzip-core" }

[[bin]]
name = "mkbinpack"
path = "src/main.rs"
```

- [ ] **Step 2: main.rs**

Crea `tools/mkbinpack/src/main.rs`:

```rust
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
```

- [ ] **Step 3: Build + roundtrip manuale**

Run:
```
wsl -d Ubuntu-22.04 -u root -e bash -lc 'source ~/.cargo/env && cd /mnt/w/Work/GitHub/ruos/tools/mkbinpack && cargo build --release 2>&1 | tail -5 && cd /tmp && printf hello > a.bin && printf world > b.bin && /mnt/w/Work/GitHub/ruos/tools/mkbinpack/target/release/mkbinpack out.bgz a.bin b.bin && ls -l out.bgz'
```
Expected: `Finished`, poi `mkbinpack: 2 entries, N bytes -> out.bgz`, `out.bgz` esiste.

- [ ] **Step 4: Commit**

```bash
git add tools/mkbinpack/Cargo.toml tools/mkbinpack/src/main.rs
git commit -m "feat(build): mkbinpack — impacchetta /bin in bin.bgz"
```

---

## Task 4: modules.rs — skip /archive/ + /rescue/, accessor

**Files:**
- Modify: `kernel/src/modules.rs`

- [ ] **Step 1: Aggiungi i prefissi**

Sotto `const PAYLOAD_PREFIX: &str = "/payload/";` (riga 21) aggiungi:

```rust
/// Module cmdline prefix per l'archivio /bin compresso (bin.bgz). Skippato dal
/// tmpfs-mount come `/payload/`; recuperato via `archive()`.
const ARCHIVE_PREFIX: &str = "/archive/";

/// Module cmdline prefix per il set rescue (shell + tool minimi). Tenuto in
/// HHDM, scritto in /bin SOLO se l'unpack di bin.bgz fallisce (`rescue_all()`).
const RESCUE_PREFIX: &str = "/rescue/";
```

- [ ] **Step 2: Estendi lo skip in mount_all**

In `mount_all`, sostituisci il blocco (righe ~42-46):

```rust
        if path.starts_with(PAYLOAD_PREFIX) {
            // Boot artifact — not a userspace file. Skip the tmpfs copy.
            payloads += 1;
            continue;
        }
```

con:

```rust
        if path.starts_with(PAYLOAD_PREFIX)
            || path.starts_with(ARCHIVE_PREFIX)
            || path.starts_with(RESCUE_PREFIX)
        {
            // Boot artifact / archivio / rescue — non file tmpfs diretti.
            payloads += 1;
            continue;
        }
```

- [ ] **Step 3: Aggiungi accessor archive() e rescue_all()**

In coda a `kernel/src/modules.rs` aggiungi:

```rust
/// Bytes del modulo archivio `/archive/<name>` (es. `bin.bgz`), HHDM-mapped e
/// valido per tutta la vita kernel (vedi `payload`). `None` se assente.
pub fn archive(name: &str) -> Option<&'static [u8]> {
    let resp = MODULES.response()?;
    for m in resp.modules() {
        if let Some(stripped) = m.cmdline().strip_prefix(ARCHIVE_PREFIX) {
            if stripped == name {
                // SAFETY: vedi `payload()` — buffer valido per la vita kernel.
                return Some(unsafe { core::mem::transmute::<&[u8], &'static [u8]>(m.data()) });
            }
        }
    }
    None
}

/// Tutti i moduli rescue come `(basename, data)` (es. `("shell.wasm", &[..])`).
/// Usati per popolare `/bin` quando bin.bgz manca/è corrotto.
pub fn rescue_all() -> alloc::vec::Vec<(&'static str, &'static [u8])> {
    let mut out = alloc::vec::Vec::new();
    if let Some(resp) = MODULES.response() {
        for m in resp.modules() {
            if let Some(name) = m.cmdline().strip_prefix(RESCUE_PREFIX) {
                // SAFETY: vedi `payload()`.
                let name = unsafe { core::mem::transmute::<&str, &'static str>(name) };
                let data = unsafe { core::mem::transmute::<&[u8], &'static [u8]>(m.data()) };
                out.push((name, data));
            }
        }
    }
    out
}
```

- [ ] **Step 4: Build-check kernel (compila)**

Run: `wsl -d Ubuntu-22.04 -u root -e bash -lc 'source ~/.cargo/env && cd /mnt/w/Work/GitHub/ruos && cargo build -p kernel 2>&1 | tail -15'`
Expected: `Finished` (warning su `archive`/`rescue_all` inutilizzati = ok, usati in Task 6).

- [ ] **Step 5: Commit**

```bash
git add kernel/src/modules.rs
git commit -m "feat(kernel): modules — prefissi /archive/ + /rescue/ e accessor"
```

---

## Task 5: kernel — dipendenza gzip-core

**Files:**
- Modify: `kernel/Cargo.toml`

- [ ] **Step 1: Aggiungi la dipendenza (no default features → no_std)**

Nella sezione `[dependencies]` di `kernel/Cargo.toml` aggiungi:

```toml
gzip-core = { path = "../user/gzip-core", default-features = false }
```

- [ ] **Step 2: Build-check**

Run: `wsl -d Ubuntu-22.04 -u root -e bash -lc 'source ~/.cargo/env && cd /mnt/w/Work/GitHub/ruos && cargo build -p kernel 2>&1 | tail -15'`
Expected: `Finished`. Se errore `gzip_core` richiede std → manca `default-features = false`.

- [ ] **Step 3: Commit**

```bash
git add kernel/Cargo.toml
git commit -m "build(kernel): dipendenza gzip-core no_std"
```

---

## Task 6: fase unpack_bin + rimozione media_bin

**Files:**
- Create: `kernel/src/boot/phases/unpack_bin.rs`
- Modify: `kernel/src/boot/phases/mod.rs:12` (mod media_bin → unpack_bin)
- Modify: `kernel/src/boot/mod.rs:25-31` (chiamata fase)
- Modify: `kernel/src/executor/mod.rs:247` e `:642-672` (rimuovi bin_overlay_task)
- Delete: `kernel/src/boot/phases/media_bin.rs`

- [ ] **Step 1: Crea unpack_bin.rs**

Crea `kernel/src/boot/phases/unpack_bin.rs`:

```rust
//! Phase — unpack_bin: popola `/bin` decomprimendo l'archivio `bin.bgz`.
//!
//! `bin.bgz` è un modulo Limine (`/archive/bin.bgz`) caricato in RAM (HHDM) dal
//! firmware UEFI: leggibile su ogni HW, indipendente da driver USB-MSC/ATAPI.
//! Qui lo parsiamo (container RBIN) e scriviamo ogni membro gzip decompresso in
//! tmpfs `/bin`. Se l'archivio manca/è corrotto → set rescue dai moduli
//! `/rescue/`. La chiavetta USB è scollegabile appena finita questa fase.

use crate::boot::BootError;
use crate::vfs::{self, OpenFlags};
use alloc::format;
use gzip_core::pack;

pub fn init() -> Result<(), BootError> {
    match crate::modules::archive("bin.bgz") {
        Some(bytes) => unpack(bytes),
        None => {
            crate::bwarn!("unpack_bin", "bin.bgz module missing → rescue fallback");
            rescue_fallback();
        }
    }
    Ok(())
}

fn unpack(bytes: &[u8]) {
    let iter = match pack::parse(bytes) {
        Ok(it) => it,
        Err(e) => {
            crate::bwarn!("unpack_bin", "bin.bgz parse error {:?} → rescue fallback", e);
            rescue_fallback();
            return;
        }
    };
    let mut ok = 0usize;
    let mut fail = 0usize;
    for entry in iter {
        match entry {
            Ok((name, gz)) => match pack::decompress_member(gz) {
                Ok(data) => {
                    let path = format!("/bin/{}", name);
                    if write_file(&path, &data).is_ok() {
                        ok += 1;
                    } else {
                        fail += 1;
                        crate::bwarn!("unpack_bin", "write {} failed", name);
                    }
                }
                Err(e) => {
                    fail += 1;
                    crate::bwarn!("unpack_bin", "{}: {}", name, e);
                }
            },
            Err(e) => {
                fail += 1;
                crate::bwarn!("unpack_bin", "archive entry error {:?}", e);
            }
        }
    }
    if ok == 0 {
        crate::bwarn!("unpack_bin", "no bins unpacked → rescue fallback");
        rescue_fallback();
        return;
    }
    crate::binfo!("unpack_bin", "unpacked {} bins from bin.bgz ({} failed)", ok, fail);
}

fn rescue_fallback() {
    let mut n = 0usize;
    for (name, data) in crate::modules::rescue_all() {
        let path = format!("/bin/{}", name);
        if write_file(&path, data).is_ok() {
            n += 1;
        }
    }
    if n == 0 {
        panic!("unpack_bin: bin.bgz unusable AND no /rescue/ modules — system has no /bin");
    }
    crate::binfo!("unpack_bin", "rescue: {} fallback bins in /bin", n);
}

fn write_file(path: &str, bytes: &[u8]) -> Result<(), vfs::VfsError> {
    vfs::block_on(async {
        let fd = vfs::open(path, OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::READ).await?;
        vfs::write(fd, bytes).await?;
        vfs::close(fd).await?;
        Ok(())
    })
}
```

- [ ] **Step 2: phases/mod.rs — sostituisci la dichiarazione mod**

In `kernel/src/boot/phases/mod.rs` riga 12, sostituisci:

```rust
pub mod media_bin;
```

con:

```rust
pub mod unpack_bin;
```

- [ ] **Step 3: boot/mod.rs — riordina le fasi**

In `kernel/src/boot/mod.rs`, nella `run()`, sposta l'unpack subito dopo `fs` e rimuovi `media_bin`. Sostituisci il blocco da `phases::fs::init()?;` (riga 25) fino a `phases::media_bin::init()?;` (riga 30) con:

```rust
    phases::fs::init()?;
    // /bin off-boot: decomprime bin.bgz (modulo Limine in HHDM) in tmpfs. Non
    // dipende da storage/usb → subito dopo fs. Elimina la dipendenza dal medium.
    phases::unpack_bin::init()?;
    phases::storage::init()?;
    // USB after the framebuffer console (devices) so its bring-up logs are
    // VISIBLE on real hardware (no serial there). USB only needs PCI; it does
    // not depend on devices/fs/storage. Must still precede userland (the
    // executor that runs usb_poll_task).
    phases::usb::init()?;
```

- [ ] **Step 4: executor — rimuovi lo spawn di bin_overlay_task**

In `kernel/src/executor/mod.rs` riga ~247 elimina la riga:

```rust
        spawner.spawn(bin_overlay_task()).unwrap();
```

- [ ] **Step 5: executor — rimuovi la def di bin_overlay_task**

In `kernel/src/executor/mod.rs` elimina l'intera funzione `async fn bin_overlay_task()` (righe ~642-672, incluso il commento doc sopra che cita `media_bin`).

- [ ] **Step 6: Elimina media_bin.rs**

```bash
git rm kernel/src/boot/phases/media_bin.rs
```

- [ ] **Step 7: Build-check kernel**

Run: `wsl -d Ubuntu-22.04 -u root -e bash -lc 'source ~/.cargo/env && cd /mnt/w/Work/GitHub/ruos && cargo build -p kernel 2>&1 | tail -25'`
Expected: `Finished`. Warning attesi: `usb::msc::first_block`, `ahci::acquire_atapi_port`, `blockdev::SectorScale`, `usb::registry::first_msc_slot` ora inutilizzati → accettati (driver USB-MSC dormiente, vedi spec). Errori `media_bin` non trovato → riferimento residuo da pulire (cerca `media_bin` con grep).

- [ ] **Step 8: Verifica nessun riferimento residuo a media_bin in codice attivo**

Run: `wsl -d Ubuntu-22.04 -u root -e bash -lc 'cd /mnt/w/Work/GitHub/ruos && grep -rn "media_bin\|bin_overlay" kernel/src/ || echo NONE'`
Expected: solo commenti residui (ahci/mod.rs, storage.rs) accettabili; nessun uso di codice (no `media_bin::`, no `bin_overlay_task`).

- [ ] **Step 9: Commit**

```bash
git add kernel/src/boot/phases/unpack_bin.rs kernel/src/boot/phases/mod.rs kernel/src/boot/mod.rs kernel/src/executor/mod.rs
git rm kernel/src/boot/phases/media_bin.rs
git commit -m "feat(kernel): fase unpack_bin (bin.bgz → /bin), rimuove media_bin"
```

---

## Task 7: limine.conf — bin.bgz + rescue set

**Files:**
- Modify: `limine.conf:11-68`

- [ ] **Step 1: Sostituisci il blocco /bin loose con archivio + rescue**

In `limine.conf`, sostituisci tutte le righe da 11 a 68 (dal commento "Live-CD fallback set" fino a `module_cmdline: /bin/which.wasm`) con:

```
    # /bin completo: archivio compresso caricato in RAM (HHDM) come modulo opaco.
    # Il kernel (fase unpack_bin) lo decomprime in tmpfs /bin dopo l'handoff.
    # Limine legge il medium via firmware UEFI → robusto su ogni HW, USB
    # scollegabile appena finita la fase. cmdline /archive/ → modules.rs lo
    # skippa dal tmpfs-mount e lo espone via modules::archive().
    module_path: boot():/bin.bgz
    module_cmdline: /archive/bin.bgz
    # Set rescue: shell + diagnostica minima, tenuti in RAM (HHDM, prefisso
    # /rescue/). Scritti in /bin SOLO se l'unpack di bin.bgz fallisce, così il
    # sistema resta raggiungibile e ispezionabile (dmesg). I file vivono in
    # /rescue/ sull'ISO (piccoli ~300KB), NON in /bin loose.
    module_path: boot():/rescue/shell.wasm
    module_cmdline: /rescue/shell.wasm
    module_path: boot():/rescue/ls.wasm
    module_cmdline: /rescue/ls.wasm
    module_path: boot():/rescue/cat.wasm
    module_cmdline: /rescue/cat.wasm
    module_path: boot():/rescue/echo.wasm
    module_cmdline: /rescue/echo.wasm
    module_path: boot():/rescue/dmesg.wasm
    module_cmdline: /rescue/dmesg.wasm
    module_path: boot():/rescue/lspci.wasm
    module_cmdline: /rescue/lspci.wasm
```

(Le righe 1-10 — kernel, init.wasm, init.sh — e 69-81 — /payload/ — restano invariate.)

- [ ] **Step 2: Verifica visiva**

Run: `wsl -d Ubuntu-22.04 -u root -e bash -lc 'cd /mnt/w/Work/GitHub/ruos && grep -n "module_cmdline" limine.conf'`
Expected: `/init.wasm`, `/etc/init.sh`, `/archive/bin.bgz`, sei `/rescue/*.wasm`, quattro `/payload/*`. Nessun `/bin/*.wasm`.

- [ ] **Step 3: Commit**

```bash
git add limine.conf
git commit -m "feat(livecd): limine.conf — bin.bgz (/archive/) + rescue (/rescue/)"
```

---

## Task 8: Makefile — pack bin.bgz, ISO, asserzioni

**Files:**
- Modify: `Makefile` (host tool mkbinpack, recipe `iso`, `run-test`, `run-test-usb`)

- [ ] **Step 1: Dichiara il tool mkbinpack (accanto a WT_PRECOMPILE, ~riga 92)**

Dopo la regola di `$(WT_PRECOMPILE)` (riga 94-95) aggiungi:

```makefile
# Host packer: /bin → bin.bgz (container RBIN di membri gzip).
MKBINPACK := tools/mkbinpack/target/release/mkbinpack
$(MKBINPACK): tools/mkbinpack/src/main.rs tools/mkbinpack/Cargo.toml user/gzip-core/src/pack.rs
	source $$HOME/.cargo/env && cd tools/mkbinpack && cargo build --release

# Set rescue: piccoli .wasm tenuti loose in /rescue sull'ISO + dentro bin.bgz.
RESCUE_TOOLS := shell ls cat echo dmesg lspci
```

- [ ] **Step 2: Aggiungi MKBINPACK ai prerequisiti di iso**

In testa alla regola `iso:` (riga 220), aggiungi `$(MKBINPACK)` alla lista prerequisiti (dopo `limine`):

```makefile
iso: build limine $(MKBINPACK) $(USER_WASMS) $(INIT_SCRIPT) build/wtecho.cwasm build/about.cwasm build/files.cwasm build/terminal.cwasm build/system.cwasm build/notepad.cwasm kernel/src/wasm/wt/reactor.cwasm kernel/src/wasm/wt/reactor_close.cwasm kernel/src/wasm/wt/probe.cwasm kernel/src/wasm/wt/egui_demo.cwasm kernel/src/wasm/wt/shell.cwasm
```

- [ ] **Step 3: Riscrivi la sezione staging /bin della recipe iso**

Nella recipe `iso` sostituisci il blocco righe 221-243 (da `rm -rf $(ISO_ROOT)` fino alla riga `-cp apps/*.cwasm ...`) con:

```makefile
	rm -rf $(ISO_ROOT) build/binstage
	mkdir -p $(ISO_ROOT)/boot/limine $(ISO_ROOT)/EFI/BOOT \
	         $(ISO_ROOT)/rescue $(ISO_ROOT)/etc $(ISO_ROOT)/root build/binstage
	cp $(KERNEL) $(ISO_ROOT)/boot/kernel
	cp limine.conf $(ISO_ROOT)/boot/limine/
	cp limine-ssd.conf $(ISO_ROOT)/boot/limine/
	for f in $(ROOT_WASMS); do cp $$f $(ISO_ROOT)/; done
	# Stage dell'intero /bin in build/binstage, poi pack → bin.bgz (non loose).
	for n in $(BIN_TOOLS); do cp user-bin/$$n.wasm build/binstage/; done
	cp build/wtecho.cwasm build/binstage/wtecho.cwasm
	cp build/about.cwasm build/binstage/about.cwasm
	cp build/files.cwasm build/binstage/files.cwasm
	cp build/terminal.cwasm build/binstage/terminal.cwasm
	cp build/system.cwasm build/binstage/system.cwasm
	cp build/notepad.cwasm build/binstage/notepad.cwasm
	cp kernel/src/wasm/wt/reactor.cwasm build/binstage/compositor.cwasm
	cp kernel/src/wasm/wt/egui_demo.cwasm build/binstage/egui-demo.cwasm
	cp kernel/src/wasm/wt/shell.cwasm build/binstage/shell.cwasm
	# App drop-folder esterne (apps/*.cwasm): nel pack se presenti. No-op se vuoto.
	-cp apps/*.cwasm build/binstage/ 2>/dev/null || true
	$(MKBINPACK) $(ISO_ROOT)/bin.bgz build/binstage/*
	# Set rescue loose in /rescue (piccolo): fallback se bin.bgz fallisce.
	for n in $(RESCUE_TOOLS); do cp user-bin/$$n.wasm $(ISO_ROOT)/rescue/; done
```

(Le righe successive — `cp $(INIT_SCRIPT) ...`, Limine artifacts, `xorriso`, `limine bios-install` — restano invariate.)

- [ ] **Step 4: Build ISO completa**

Run: `wsl -d Ubuntu-22.04 -u root -e bash -lc 'source ~/.cargo/env && cd /mnt/w/Work/GitHub/ruos && make iso 2>&1 | tail -25'`
Expected: log `mkbinpack: N entries, M bytes -> build/iso_root/bin.bgz`, poi ISO assemblata. Verifica: `ls -l build/iso_root/bin.bgz` esiste, `build/iso_root/bin/` NON esiste, `build/iso_root/rescue/` ha 6 file.

- [ ] **Step 5: Aggiorna le asserzioni run-test (livecd /bin)**

In `run-test` (righe 282-283) sostituisci le due righe ATAPI/overlay:

```makefile
		grep -qE "ahci port [0-9]+ atapi sectors=" build/serial.log || { echo TEST_FAIL_ATAPI; exit 1; }; \
		grep -qF "/bin overlaid from ISO9660" build/serial.log || { echo TEST_FAIL_LIVECD_BIN; exit 1; }; \
```

con:

```makefile
		grep -qE "unpacked [0-9]+ bins from bin.bgz" build/serial.log || { echo TEST_FAIL_UNPACK_BIN; exit 1; }; \
```

- [ ] **Step 6: Aggiorna run-test-usb (boot da USB usa lo stesso bin.bgz)**

In `run-test-usb` sostituisci le asserzioni MSC (righe 308-309):

```makefile
		grep -qE "msc  MSC ready slot=" build/serial-usb.log || { echo TEST_FAIL_MSC_ENUM; exit 1; }; \
		grep -qF "/bin overlaid from USB-MSC" build/serial-usb.log || { echo TEST_FAIL_USB_BIN; exit 1; }; \
```

con:

```makefile
		grep -qE "unpacked [0-9]+ bins from bin.bgz" build/serial-usb.log || { echo TEST_FAIL_UNPACK_BIN; exit 1; }; \
```

Aggiorna anche il commento sopra `run-test-usb` (righe 292-296) per riflettere che ora il boot da USB carica `bin.bgz` via Limine (firmware), non più via driver USB-MSC.

- [ ] **Step 7: Gate QEMU (cdrom)**

Run: `wsl -d Ubuntu-22.04 -u root -e bash -lc 'source ~/.cargo/env && cd /mnt/w/Work/GitHub/ruos && make run-test 2>&1 | tail -20'`
Expected: `TEST_PASS`. In log: `unpacked N bins from bin.bgz`, shell pronta (`$(HELLO)`).

- [ ] **Step 8: Gate QEMU (usb-storage)**

Run: `wsl -d Ubuntu-22.04 -u root -e bash -lc 'source ~/.cargo/env && cd /mnt/w/Work/GitHub/ruos && make run-test-usb 2>&1 | tail -20'`
Expected: `TEST_PASS_USB`. In log: `unpacked N bins from bin.bgz` anche bootando da chiavetta.

- [ ] **Step 9: Commit**

```bash
git add Makefile
git commit -m "build(livecd): pack bin.bgz nell'ISO, rescue loose, gate unpack_bin"
```

---

## Task 9: Changelog + verifica RAM

**Files:**
- Create: `CHANGELOG/NN-26-06-10-bin-pack-livecd-impl.md` (NN = numero più alto in `CHANGELOG/` + 1; al momento della stesura del piano il prossimo libero è 406 — riverifica con `ls CHANGELOG/ | sort | tail -1`)

- [ ] **Step 1: Verifica peak RAM a runtime (free in shell)**

Run interattivo: `wsl -d Ubuntu-22.04 -u root -e bash -lc 'source ~/.cargo/env && cd /mnt/w/Work/GitHub/ruos && make run'`
In shell ruos: `free`. Verifica heap usato < 384MB con margine (atteso ~132MB tmpfs + overhead). Se vicino al limite → annota per follow-up (es. bump heap o strategia C). Chiudi QEMU.

- [ ] **Step 2: Crea la changelog entry**

Crea `CHANGELOG/<NN>-26-06-10-bin-pack-livecd-impl.md` (NN = prossimo libero):

```markdown
# <NN> — Live-CD /bin via bin.bgz (archivio compresso caricato da Limine)

**Data:** 2026-06-10

## Cosa
`/bin` ora popolato dalla nuova fase boot `unpack_bin`, che decomprime un singolo
archivio `bin.bgz` (container RBIN di membri gzip) caricato da Limine come modulo
in HHDM. Rimuove la fase `media_bin` (ATAPI + USB-MSC overlay) e il task deferred
`bin_overlay_task`. gzip-core reso no_std (feature `std` per i bin userland);
nuovo modulo `pack`; nuovo tool host `mkbinpack`; ISO ship solo `bin.bgz` +
set rescue loose in `/rescue`. Driver USB-MSC resta dormiente.

## Perché
Il /bin off-boot da USB-MSC/ATAPI era fragile su HW reale (xHCI, port-power,
timing). Limine legge il medium via firmware UEFI su ogni HW: caricare un blob
compresso e decomprimerlo nel kernel elimina la dipendenza runtime e rende la
chiavetta USB scollegabile. Spec/piano: docs/superpowers/{specs,plans}/2026-06-10.

## File toccati
- user/gzip-core/{Cargo.toml,src/lib.rs,src/format.rs,src/pack.rs}
- tools/mkbinpack/{Cargo.toml,src/main.rs}
- kernel/src/boot/phases/unpack_bin.rs (nuovo), media_bin.rs (rimosso)
- kernel/src/boot/{mod.rs,phases/mod.rs}, kernel/src/modules.rs
- kernel/src/executor/mod.rs, kernel/Cargo.toml
- limine.conf, Makefile
```

- [ ] **Step 3: Commit**

```bash
git add CHANGELOG/<NN>-26-06-10-bin-pack-livecd-impl.md
git commit -m "docs(changelog): <NN> bin.bgz live-CD impl"
```

---

## Self-Review (eseguito durante la stesura)

**Spec coverage:** formato RBIN (Task 2), gzip-core no_std (Task 1), mkbinpack host (Task 3), fase unpack_bin + rescue fallback (Task 6), drop media_bin + USB-MSC dormiente (Task 6), limine.conf /archive/+/rescue/ (Task 7), ISO solo bin.bgz + build-iso.ps1 invariato (Task 8, ps1 non toccato), peak RAM (Task 9 Step 1). Tutte le sezioni della spec coperte.

**Note implementative:**
- `archive()` riusa il pattern `payload()` ma con prefisso dedicato `/archive/` (la spec lasciava aperta la scelta /payload/ vs /archive/: scelto /archive/ per non sporcare la semantica installer-SSD di /payload/).
- I file rescue restano loose in `/rescue` sull'ISO (Limine deve poterli aprire come moduli); "ISO solo bin.bgz" si riferisce al /bin grande (132MB), non ai ~300KB di rescue.
- Warning kernel attesi su helper USB-MSC ora inutilizzati: accettati (driver dormiente), coerente con la decisione "rimozione = YAGNI" della spec.

**Type consistency:** `write_archive`/`parse`/`decompress_member`/`PackError` usati identici in pack.rs, mkbinpack, unpack_bin. `modules::archive`/`modules::rescue_all` usati in unpack_bin con le firme definite in Task 4.
