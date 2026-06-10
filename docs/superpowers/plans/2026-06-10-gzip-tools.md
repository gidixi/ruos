# gzip / gunzip / zcat Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Tre tool userland `gzip`/`gunzip`/`zcat` (wasm32-wasip1, eseguiti da wasmi) con formato gzip RFC 1952 via `miniz_oxide`, semantica Unix classica.

**Architecture:** Lib condivisa `user/gzip-core` (CRC32 + header/trailer gzip + compress/decompress + CLI condivisa) e tre bin thin che la chiamano con default diversi. Wiring: membri workspace `user/Cargo.toml` + `BIN_TOOLS` nel Makefile + sezione smoke in `user-bin/smoke.sh` con assert in `make run-test`. Zero kernel.

**Tech Stack:** Rust std su wasm32-wasip1, `miniz_oxide` 0.8 (pure Rust), `ruos-rt` per il cwd sync. Spec: `docs/superpowers/specs/2026-06-10-gzip-tools-design.md`.

**Ambiente (QUESTA macchina, override CLAUDE.md):**
- Test host (unit test gzip-core): cargo Windows stabile, da `W:\Work\GitHub\ruos\user`: `cargo test -p gzip-core`. Fallback WSL: `wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos/user && cargo test -p gzip-core'`.
- Build wasm/ISO/QEMU: solo WSL `Ubuntu-22.04`, repo a `/mnt/w/Work/GitHub/ruos`: `wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && <cmd>'`.
- **Commit: SOLO se l'utente li ha richiesti** (regola CLAUDE.md). Se autorizzati: creare prima branch `feat/gzip-tools` (siamo su `main`). Gli step "Commit" sotto si saltano se non autorizzati.
- Ogni task che tocca il repo → al termine del piano una entry CHANGELOG riassuntiva (Task 9).

---

## File Structure

```
user/Cargo.toml                  MODIFY  + membri gzip-core gzip gunzip zcat
user/gzip-core/Cargo.toml        CREATE  lib, dep miniz_oxide
user/gzip-core/src/lib.rs        CREATE  mod wiring + re-export API
user/gzip-core/src/crc32.rs      CREATE  CRC32 (tabella const)
user/gzip-core/src/format.rs     CREATE  compress/decompress + GzError
user/gzip-core/src/cli.rs        CREATE  parse_args/output_path/run_cli
user/gzip/Cargo.toml             CREATE  bin thin
user/gzip/src/main.rs            CREATE
user/gunzip/Cargo.toml           CREATE  bin thin
user/gunzip/src/main.rs          CREATE
user/zcat/Cargo.toml             CREATE  bin thin
user/zcat/src/main.rs            CREATE
Makefile                         MODIFY  BIN_TOOLS + assert run-test
user-bin/smoke.sh                MODIFY  sezione gzip smoke
CHANGELOG/4NN-26-06-10-*.md      CREATE  entry implementazione
```

---

### Task 1: Scaffold gzip-core nel workspace

**Files:**
- Create: `user/gzip-core/Cargo.toml`, `user/gzip-core/src/lib.rs`
- Modify: `user/Cargo.toml`

- [ ] **Step 1: Crea `user/gzip-core/Cargo.toml`**

```toml
[package]
name = "gzip-core"
version = "0.1.0"
edition = "2021"

[dependencies]
miniz_oxide = "0.8"

[lib]
name = "gzip_core"
path = "src/lib.rs"
```

- [ ] **Step 2: Crea `user/gzip-core/src/lib.rs`** (scheletro, i moduli arrivano nei task dopo)

```rust
//! gzip-core — formato gzip (RFC 1952) + deflate via miniz_oxide.
//! Logica condivisa dai tool gzip/gunzip/zcat.
```

- [ ] **Step 3: Aggiungi i membri al workspace `user/Cargo.toml`**

Nella lista `members`, dopo la riga `"ruos-rt",` aggiungi:

```toml
    "gzip-core", "gzip", "gunzip", "zcat",
```

NB: `gzip`/`gunzip`/`zcat` non esistono ancora → cargo fallirebbe. Per questo task aggiungi SOLO `"gzip-core",`; gli altri tre si aggiungono nel Task 6 quando le crate esistono.

- [ ] **Step 4: Verifica**

Run (da `W:\Work\GitHub\ruos\user`): `cargo check -p gzip-core`
Expected: `Finished` senza errori (scarica miniz_oxide, aggiorna Cargo.lock).

- [ ] **Step 5: Commit** (solo se autorizzato)

```bash
git add user/Cargo.toml user/Cargo.lock user/gzip-core
git commit -m "feat(user): scaffold gzip-core crate"
```

---

### Task 2: CRC32

**Files:**
- Create: `user/gzip-core/src/crc32.rs`
- Modify: `user/gzip-core/src/lib.rs`

- [ ] **Step 1: Scrivi `user/gzip-core/src/crc32.rs` con i test PRIMA dell'implementazione** (test in fondo al file; l'implementazione iniziale è `todo!()`)

```rust
//! CRC-32 (IEEE 802.3, polinomio riflesso 0xEDB88320) — quello del trailer gzip.

const TABLE: [u32; 256] = make_table();

const fn make_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

pub fn crc32(data: &[u8]) -> u32 {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_zero() {
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn check_vector() {
        // Vettore di verifica standard CRC-32/ISO-HDLC.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn hello() {
        // gzip -n <<< noto: crc32("hello") = 0x3610A686
        assert_eq!(crc32(b"hello"), 0x3610_A686);
    }
}
```

- [ ] **Step 2: Registra il modulo in `lib.rs`** (aggiungi sotto il doc comment)

```rust
mod crc32;
```

- [ ] **Step 3: Verifica che i test falliscano**

Run: `cargo test -p gzip-core crc32`
Expected: FAIL (panic `not yet implemented`).

- [ ] **Step 4: Implementa `crc32`** (sostituisci il `todo!()`)

```rust
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}
```

- [ ] **Step 5: Verifica che i test passino**

Run: `cargo test -p gzip-core crc32`
Expected: `3 passed`.

- [ ] **Step 6: Commit** (solo se autorizzato)

```bash
git add user/gzip-core
git commit -m "feat(gzip-core): crc32 table-based"
```

---

### Task 3: compress()

**Files:**
- Create: `user/gzip-core/src/format.rs`
- Modify: `user/gzip-core/src/lib.rs`

- [ ] **Step 1: Crea `user/gzip-core/src/format.rs`** con tipi, firma `compress` (`todo!()`) e test

```rust
//! Formato gzip RFC 1952: header (10 byte + campi opzionali FLG), payload
//! deflate raw, trailer CRC32+ISIZE little-endian.

use core::fmt;

use crate::crc32::crc32;

pub const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];
const CM_DEFLATE: u8 = 8;
const FHCRC: u8 = 2;
const FEXTRA: u8 = 4;
const FNAME: u8 = 8;
const FCOMMENT: u8 = 16;

#[derive(Debug, PartialEq, Eq)]
pub enum GzError {
    NotGzip,
    TruncatedHeader,
    TruncatedTrailer,
    BadDeflate,
    CrcMismatch,
    SizeMismatch,
    TrailingGarbage,
}

impl fmt::Display for GzError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            GzError::NotGzip => "not in gzip format",
            GzError::TruncatedHeader => "truncated gzip header",
            GzError::TruncatedTrailer => "truncated gzip trailer",
            GzError::BadDeflate => "invalid compressed data",
            GzError::CrcMismatch => "crc error",
            GzError::SizeMismatch => "length error",
            GzError::TrailingGarbage => "trailing garbage after compressed data",
        })
    }
}

/// Comprimi `data` in un member gzip completo. `level` 1..=9 (clamp), 6 = default.
pub fn compress(data: &[u8], level: u8) -> Vec<u8> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_is_canonical() {
        let gz = compress(b"hello", 6);
        // magic, CM=deflate, FLG=0, MTIME=0
        assert_eq!(&gz[..8], &[0x1f, 0x8b, 8, 0, 0, 0, 0, 0]);
        assert_eq!(gz[9], 255); // OS = unknown
    }

    #[test]
    fn trailer_has_crc_and_isize() {
        let gz = compress(b"hello", 6);
        let n = gz.len();
        assert_eq!(&gz[n - 8..n - 4], &crc32(b"hello").to_le_bytes());
        assert_eq!(&gz[n - 4..], &5u32.to_le_bytes());
    }

    #[test]
    fn level_is_clamped() {
        // 0 e 200 non devono panicare; output comunque un gz valido (header ok).
        assert_eq!(&compress(b"x", 0)[..2], &GZIP_MAGIC);
        assert_eq!(&compress(b"x", 200)[..2], &GZIP_MAGIC);
    }

    #[test]
    fn empty_input_ok() {
        let gz = compress(b"", 6);
        assert_eq!(&gz[..2], &GZIP_MAGIC);
        assert_eq!(&gz[gz.len() - 4..], &0u32.to_le_bytes());
    }
}
```

- [ ] **Step 2: Registra il modulo e l'export in `lib.rs`**

```rust
mod format;

pub use format::{compress, decompress, GzError};
```

NB: `decompress` non esiste ancora → per ORA esporta solo `compress` e `GzError`; aggiungerai `decompress` all'export nel Task 4.

- [ ] **Step 3: Verifica che i test falliscano**

Run: `cargo test -p gzip-core format`
Expected: FAIL (`not yet implemented`).

- [ ] **Step 4: Implementa `compress`**

```rust
pub fn compress(data: &[u8], level: u8) -> Vec<u8> {
    let level = level.clamp(1, 9);
    let mut out = Vec::with_capacity(data.len() / 2 + 32);
    out.extend_from_slice(&GZIP_MAGIC);
    out.push(CM_DEFLATE);
    out.push(0); // FLG: nessun campo opzionale
    out.extend_from_slice(&[0; 4]); // MTIME = 0 (no clock affidabile)
    out.push(match level {
        9 => 2, // XFL: max compression
        1 => 4, // XFL: fastest
        _ => 0,
    });
    out.push(255); // OS = unknown
    out.extend_from_slice(&miniz_oxide::deflate::compress_to_vec(data, level));
    out.extend_from_slice(&crc32(data).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}
```

- [ ] **Step 5: Verifica che i test passino**

Run: `cargo test -p gzip-core format`
Expected: `4 passed`.

- [ ] **Step 6: Commit** (solo se autorizzato)

```bash
git add user/gzip-core
git commit -m "feat(gzip-core): gzip compress (header+deflate+trailer)"
```

---

### Task 4: decompress()

**Files:**
- Modify: `user/gzip-core/src/format.rs`, `user/gzip-core/src/lib.rs`

- [ ] **Step 1: Genera il vettore golden con gzip reale** (serve per il test cross-check)

Run:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c "printf 'hello ruos' | gzip -n -6 | od -An -v -tx1"
```
Expected: una sequenza esadecimale che inizia con `1f 8b 08 00`. Trascrivila nel test `golden_real_gzip` sotto (formato `0x1f, 0x8b, ...`).

- [ ] **Step 2: Aggiungi firme (`todo!()`) e test a `format.rs`**

Firme da aggiungere (sopra `mod tests`):

```rust
/// Decomprimi un singolo member gzip. Errore se header/trailer invalidi,
/// CRC/ISIZE non tornano, o restano byte dopo il trailer (multi-member non
/// supportato → TrailingGarbage).
pub fn decompress(data: &[u8]) -> Result<Vec<u8>, GzError> {
    todo!()
}

/// Lunghezza header: 10 byte fissi + campi opzionali da FLG (FEXTRA, FNAME,
/// FCOMMENT, FHCRC) da saltare.
fn header_len(data: &[u8]) -> Result<usize, GzError> {
    todo!()
}

fn skip_zstr(data: &[u8], pos: usize) -> Result<usize, GzError> {
    todo!()
}

/// Inflate raw che riporta anche i byte consumati (serve per localizzare il
/// trailer e rifiutare garbage dopo di esso).
fn inflate_raw(input: &[u8]) -> Result<(Vec<u8>, usize), GzError> {
    todo!()
}
```

Test da aggiungere dentro `mod tests`:

```rust
    #[test]
    fn roundtrip_levels() {
        let cases: &[&[u8]] = &[b"", b"hello ruos", &[0xAAu8; 100_000], b"abcabcabcabcabc"];
        for data in cases {
            for level in [1u8, 6, 9] {
                assert_eq!(decompress(&compress(data, level)).unwrap(), *data);
            }
        }
    }

    #[test]
    fn roundtrip_big_pseudorandom() {
        // ~1 MiB pseudo-random deterministico (xorshift), niente Math/rand dep.
        let mut x = 0x12345678u32;
        let data: Vec<u8> = (0..1_048_576)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect();
        assert_eq!(decompress(&compress(&data, 6)).unwrap(), data);
    }

    #[test]
    fn golden_real_gzip() {
        // `printf 'hello ruos' | gzip -n -6` su Ubuntu (Step 1). INCOLLA i byte:
        const GZ: &[u8] = &[
            0x1f, 0x8b, 0x08, 0x00, /* ... resto dell'output di od ... */
        ];
        assert_eq!(decompress(GZ).unwrap(), b"hello ruos");
    }

    #[test]
    fn skips_fname() {
        let gz = compress(b"abc", 6);
        let mut v = Vec::new();
        v.extend_from_slice(&gz[..3]);
        v.push(8); // FLG = FNAME
        v.extend_from_slice(&gz[4..10]);
        v.extend_from_slice(b"file.txt\0");
        v.extend_from_slice(&gz[10..]);
        assert_eq!(decompress(&v).unwrap(), b"abc");
    }

    #[test]
    fn skips_fextra() {
        let gz = compress(b"abc", 6);
        let mut v = Vec::new();
        v.extend_from_slice(&gz[..3]);
        v.push(4); // FLG = FEXTRA
        v.extend_from_slice(&gz[4..10]);
        v.extend_from_slice(&3u16.to_le_bytes()); // XLEN = 3
        v.extend_from_slice(&[1, 2, 3]);
        v.extend_from_slice(&gz[10..]);
        assert_eq!(decompress(&v).unwrap(), b"abc");
    }

    #[test]
    fn rejects_not_gzip() {
        assert_eq!(decompress(b"plain text").unwrap_err(), GzError::NotGzip);
        assert_eq!(decompress(b"").unwrap_err(), GzError::NotGzip);
    }

    #[test]
    fn rejects_truncated() {
        let gz = compress(b"hello ruos", 6);
        assert_eq!(decompress(&gz[..5]).unwrap_err(), GzError::TruncatedHeader);
        // header ok ma manca il trailer
        assert!(decompress(&gz[..gz.len() - 8]).is_err());
    }

    #[test]
    fn rejects_bad_crc() {
        let mut gz = compress(b"hello ruos", 6);
        let n = gz.len();
        gz[n - 8] ^= 0xFF; // corrompi il CRC nel trailer
        assert_eq!(decompress(&gz).unwrap_err(), GzError::CrcMismatch);
    }

    #[test]
    fn rejects_bad_isize() {
        let mut gz = compress(b"hello ruos", 6);
        let n = gz.len();
        gz[n - 1] ^= 0xFF; // corrompi ISIZE
        assert_eq!(decompress(&gz).unwrap_err(), GzError::SizeMismatch);
    }

    #[test]
    fn rejects_trailing_garbage_and_multimember() {
        let mut gz = compress(b"abc", 6);
        gz.push(0x00);
        assert_eq!(decompress(&gz).unwrap_err(), GzError::TrailingGarbage);
        let two: Vec<u8> = [compress(b"a", 6), compress(b"b", 6)].concat();
        assert_eq!(decompress(&two).unwrap_err(), GzError::TrailingGarbage);
    }

    #[test]
    fn rejects_corrupt_deflate() {
        let mut gz = compress(b"hello ruos hello ruos", 6);
        gz[12] ^= 0xFF; // corrompi il payload deflate
        // Qualsiasi errore va bene (BadDeflate o Crc), MAI panic.
        assert!(decompress(&gz).is_err());
    }
```

- [ ] **Step 3: Aggiorna l'export in `lib.rs`** — assicurati che sia `pub use format::{compress, decompress, GzError};`

- [ ] **Step 4: Verifica che i nuovi test falliscano**

Run: `cargo test -p gzip-core format`
Expected: FAIL (`not yet implemented`) sui nuovi test, i 4 di compress restano verdi.

- [ ] **Step 5: Implementa**

```rust
pub fn decompress(data: &[u8]) -> Result<Vec<u8>, GzError> {
    let hlen = header_len(data)?;
    if data.len() < hlen + 8 {
        return Err(GzError::TruncatedTrailer);
    }
    let (out, consumed) = inflate_raw(&data[hlen..])?;
    let trailer = &data[hlen + consumed..];
    if trailer.len() < 8 {
        return Err(GzError::TruncatedTrailer);
    }
    if trailer.len() > 8 {
        return Err(GzError::TrailingGarbage);
    }
    let crc = u32::from_le_bytes(trailer[..4].try_into().unwrap());
    let isize_ = u32::from_le_bytes(trailer[4..].try_into().unwrap());
    if crc32(&out) != crc {
        return Err(GzError::CrcMismatch);
    }
    if out.len() as u32 != isize_ {
        return Err(GzError::SizeMismatch);
    }
    Ok(out)
}

fn header_len(data: &[u8]) -> Result<usize, GzError> {
    if data.len() < 2 || data[..2] != GZIP_MAGIC {
        return Err(GzError::NotGzip);
    }
    if data.len() < 10 {
        return Err(GzError::TruncatedHeader);
    }
    if data[2] != CM_DEFLATE {
        return Err(GzError::NotGzip);
    }
    let flg = data[3];
    let mut pos = 10;
    if flg & FEXTRA != 0 {
        if data.len() < pos + 2 {
            return Err(GzError::TruncatedHeader);
        }
        let xlen = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2 + xlen;
    }
    if flg & FNAME != 0 {
        pos = skip_zstr(data, pos)?;
    }
    if flg & FCOMMENT != 0 {
        pos = skip_zstr(data, pos)?;
    }
    if flg & FHCRC != 0 {
        pos += 2;
    }
    if data.len() < pos {
        return Err(GzError::TruncatedHeader);
    }
    Ok(pos)
}

fn skip_zstr(data: &[u8], mut pos: usize) -> Result<usize, GzError> {
    while pos < data.len() {
        pos += 1;
        if data[pos - 1] == 0 {
            return Ok(pos);
        }
    }
    Err(GzError::TruncatedHeader)
}

fn inflate_raw(input: &[u8]) -> Result<(Vec<u8>, usize), GzError> {
    use miniz_oxide::inflate::stream::{inflate, InflateState};
    use miniz_oxide::{DataFormat, MZFlush, MZStatus};

    let mut state = InflateState::new_boxed(DataFormat::Raw);
    let mut out = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut consumed = 0;
    loop {
        let res = inflate(&mut state, &input[consumed..], &mut buf, MZFlush::None);
        consumed += res.bytes_consumed;
        out.extend_from_slice(&buf[..res.bytes_written]);
        match res.status {
            Ok(MZStatus::StreamEnd) => return Ok((out, consumed)),
            Ok(MZStatus::Ok) => {
                // Nessun progresso = stream deflate troncato/invalido.
                if res.bytes_consumed == 0 && res.bytes_written == 0 {
                    return Err(GzError::BadDeflate);
                }
            }
            _ => return Err(GzError::BadDeflate),
        }
    }
}
```

- [ ] **Step 6: Verifica che i test passino**

Run: `cargo test -p gzip-core`
Expected: tutti verdi (3 crc32 + 4 compress + 11 decompress).

- [ ] **Step 7: Commit** (solo se autorizzato)

```bash
git add user/gzip-core
git commit -m "feat(gzip-core): gzip decompress (header parse, inflate, crc/isize check)"
```

---

### Task 5: CLI — parse_args e output_path

**Files:**
- Create: `user/gzip-core/src/cli.rs`
- Modify: `user/gzip-core/src/lib.rs`

- [ ] **Step 1: Crea `user/gzip-core/src/cli.rs`** con tipi, firme `todo!()` e test

```rust
//! CLI condivisa da gzip/gunzip/zcat: parsing flag, naming file di output,
//! dispatch file/stdin. I tre bin differiscono solo nei `Defaults`.

use std::io::{self, Read, Write};

use crate::format::{compress, decompress};

/// Default per-tool: gzip = {false,false,false}, gunzip = {true,false,false},
/// zcat = {true,true,true}.
#[derive(Clone, Copy)]
pub struct Defaults {
    pub decompress: bool,
    pub to_stdout: bool,
    pub keep: bool,
}

pub struct Opts {
    pub decompress: bool,
    pub to_stdout: bool,
    pub keep: bool,
    pub level: u8,
    pub help: bool,
    pub files: Vec<String>,
}

/// Parsa gli argomenti (senza argv[0]). Flag: -c -k -d -h -1..-9, anche in
/// cluster (`-dc`). Tutto il resto = file.
pub fn parse_args(args: &[String], d: Defaults) -> Result<Opts, String> {
    todo!()
}

/// Nome del file di output in file-mode: comprimi → aggiungi `.gz` (errore se
/// già presente), decomprimi → togli `.gz` (errore se assente).
pub fn output_path(path: &str, decompress: bool) -> Result<String, String> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GZIP_D: Defaults = Defaults { decompress: false, to_stdout: false, keep: false };
    const ZCAT_D: Defaults = Defaults { decompress: true, to_stdout: true, keep: true };

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn defaults_pass_through() {
        let o = parse_args(&[], ZCAT_D).unwrap();
        assert!(o.decompress && o.to_stdout && o.keep);
        assert_eq!(o.level, 6);
        assert!(o.files.is_empty());
    }

    #[test]
    fn flags_and_files() {
        let o = parse_args(&args(&["-dck", "a.gz", "b.gz"]), GZIP_D).unwrap();
        assert!(o.decompress && o.to_stdout && o.keep);
        assert_eq!(o.files, vec!["a.gz", "b.gz"]);
    }

    #[test]
    fn level_flag() {
        assert_eq!(parse_args(&args(&["-9"]), GZIP_D).unwrap().level, 9);
        assert_eq!(parse_args(&args(&["-1c"]), GZIP_D).unwrap().level, 1);
    }

    #[test]
    fn help_flag() {
        assert!(parse_args(&args(&["-h"]), GZIP_D).unwrap().help);
    }

    #[test]
    fn unknown_flag_is_error() {
        assert!(parse_args(&args(&["-x"]), GZIP_D).is_err());
    }

    #[test]
    fn output_path_compress() {
        assert_eq!(output_path("f.txt", false).unwrap(), "f.txt.gz");
        assert!(output_path("f.txt.gz", false).is_err()); // già .gz
    }

    #[test]
    fn output_path_decompress() {
        assert_eq!(output_path("f.txt.gz", true).unwrap(), "f.txt");
        assert!(output_path("f.txt", true).is_err()); // suffisso assente
    }
}
```

- [ ] **Step 2: Registra il modulo in `lib.rs`**

```rust
pub mod cli;
```

- [ ] **Step 3: Verifica che i test falliscano**

Run: `cargo test -p gzip-core cli`
Expected: FAIL (`not yet implemented`).

- [ ] **Step 4: Implementa `parse_args` e `output_path`**

```rust
pub fn parse_args(args: &[String], d: Defaults) -> Result<Opts, String> {
    let mut o = Opts {
        decompress: d.decompress,
        to_stdout: d.to_stdout,
        keep: d.keep,
        level: 6,
        help: false,
        files: Vec::new(),
    };
    for a in args {
        if a.len() >= 2 && a.starts_with('-') {
            for c in a[1..].chars() {
                match c {
                    'c' => o.to_stdout = true,
                    'k' => o.keep = true,
                    'd' => o.decompress = true,
                    'h' => o.help = true,
                    '1'..='9' => o.level = c as u8 - b'0',
                    _ => return Err(format!("unknown option -{c}")),
                }
            }
        } else {
            o.files.push(a.clone());
        }
    }
    Ok(o)
}

pub fn output_path(path: &str, decompress: bool) -> Result<String, String> {
    if decompress {
        path.strip_suffix(".gz")
            .map(str::to_string)
            .ok_or_else(|| "unknown suffix -- ignored".to_string())
    } else if path.ends_with(".gz") {
        Err("already has .gz suffix -- unchanged".to_string())
    } else {
        Ok(format!("{path}.gz"))
    }
}
```

- [ ] **Step 5: Verifica che i test passino**

Run: `cargo test -p gzip-core`
Expected: tutti verdi (i precedenti + 7 cli).

- [ ] **Step 6: Commit** (solo se autorizzato)

```bash
git add user/gzip-core
git commit -m "feat(gzip-core): cli arg parsing + output naming"
```

---

### Task 6: run_cli + i tre bin

**Files:**
- Modify: `user/gzip-core/src/cli.rs`, `user/Cargo.toml`
- Create: `user/gzip/Cargo.toml`, `user/gzip/src/main.rs`, `user/gunzip/Cargo.toml`, `user/gunzip/src/main.rs`, `user/zcat/Cargo.toml`, `user/zcat/src/main.rs`

- [ ] **Step 1: Aggiungi `run_cli` e helper a `cli.rs`** (sopra `mod tests`; niente test host — è I/O thin su logica già testata)

```rust
/// Entrypoint condiviso dai bin. `tool` = nome per i messaggi d'errore.
pub fn run_cli(tool: &str, d: Defaults) -> ! {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opts = match parse_args(&args, d) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{tool}: {e}");
            usage(tool);
            std::process::exit(1);
        }
    };
    if opts.help {
        usage(tool);
        std::process::exit(0);
    }
    let mut failed = false;
    if opts.files.is_empty() {
        // Niente file: stdin -> stdout.
        if let Err(e) = run_stream(&opts) {
            eprintln!("{tool}: {e}");
            failed = true;
        }
    } else {
        for f in &opts.files {
            if let Err(e) = run_file(f, &opts) {
                eprintln!("{tool}: {f}: {e}");
                failed = true; // un errore non blocca i file successivi
            }
        }
    }
    std::process::exit(if failed { 1 } else { 0 });
}

fn usage(tool: &str) {
    eprintln!("usage: {tool} [-cdkh] [-1..-9] [file ...]");
    eprintln!("  -c  write to stdout, keep input");
    eprintln!("  -d  decompress");
    eprintln!("  -k  keep input file");
    eprintln!("  -1..-9  compression level (default 6)");
    eprintln!("with no file, reads stdin and writes stdout");
}

fn transform(input: &[u8], o: &Opts) -> Result<Vec<u8>, String> {
    if o.decompress {
        decompress(input).map_err(|e| e.to_string())
    } else {
        Ok(compress(input, o.level))
    }
}

fn run_stream(o: &Opts) -> Result<(), String> {
    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input).map_err(|e| e.to_string())?;
    let out = transform(&input, o)?;
    io::stdout().write_all(&out).map_err(|e| e.to_string())
}

fn run_file(path: &str, o: &Opts) -> Result<(), String> {
    // -c: trasforma e stampa, non tocca il filesystem (suffisso irrilevante).
    if o.to_stdout {
        let data = std::fs::read(path).map_err(|e| e.to_string())?;
        let out = transform(&data, o)?;
        return io::stdout().write_all(&out).map_err(|e| e.to_string());
    }
    let out_path = output_path(path, o.decompress)?;
    if std::fs::metadata(&out_path).is_ok() {
        return Err(format!("{out_path} already exists"));
    }
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let out = transform(&data, o)?;
    std::fs::write(&out_path, &out).map_err(|e| e.to_string())?;
    if !o.keep {
        std::fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    Ok(())
}
```

- [ ] **Step 2: Crea le tre crate bin.** Per ognuna, `Cargo.toml` (qui `gzip`; per `gunzip`/`zcat` cambia `name` nei due punti):

```toml
[package]
name = "gzip"
version = "0.1.0"
edition = "2021"

[dependencies]
ruos-rt = { path = "../ruos-rt" }
gzip-core = { path = "../gzip-core" }

[[bin]]
name = "gzip"
path = "src/main.rs"
```

`user/gzip/src/main.rs`:

```rust
use gzip_core::cli::{run_cli, Defaults};

fn main() {
    ruos_rt::init(); // sync libc cwd from PWD so relative paths honor the shell's cwd
    run_cli("gzip", Defaults { decompress: false, to_stdout: false, keep: false });
}
```

`user/gunzip/src/main.rs`:

```rust
use gzip_core::cli::{run_cli, Defaults};

fn main() {
    ruos_rt::init(); // sync libc cwd from PWD so relative paths honor the shell's cwd
    run_cli("gunzip", Defaults { decompress: true, to_stdout: false, keep: false });
}
```

`user/zcat/src/main.rs`:

```rust
use gzip_core::cli::{run_cli, Defaults};

fn main() {
    ruos_rt::init(); // sync libc cwd from PWD so relative paths honor the shell's cwd
    run_cli("zcat", Defaults { decompress: true, to_stdout: true, keep: true });
}
```

- [ ] **Step 3: Aggiungi i membri a `user/Cargo.toml`** — la riga del Task 1 diventa:

```toml
    "gzip-core", "gzip", "gunzip", "zcat",
```

- [ ] **Step 4: Verifica build host + test**

Run: `cargo test -p gzip-core` poi `cargo check -p gzip -p gunzip -p zcat`
Expected: test verdi; check `Finished` (su host Windows `ruos-rt` compila: è solo std).

- [ ] **Step 5: Verifica build wasm**

Run:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos/user && source $HOME/.cargo/env && cargo build --target wasm32-wasip1 --release -p gzip -p gunzip -p zcat'
```
Expected: `Finished`; esistono `user/target/wasm32-wasip1/release/{gzip,gunzip,zcat}.wasm`.

- [ ] **Step 6: Commit** (solo se autorizzato)

```bash
git add user/Cargo.toml user/Cargo.lock user/gzip-core user/gzip user/gunzip user/zcat
git commit -m "feat(user): gzip/gunzip/zcat tools on gzip-core"
```

---

### Task 7: Wiring Makefile

**Files:**
- Modify: `Makefile` (lista `BIN_TOOLS`, ~riga 19-31)

- [ ] **Step 1: Aggiungi i tre tool a `BIN_TOOLS`.** L'ultima riga della lista:

```make
              mkdisk mkboot install umount disks wifiscan wificonnect
```

diventa:

```make
              mkdisk mkboot install umount disks wifiscan wificonnect \
              gzip gunzip zcat
```

- [ ] **Step 2: Verifica che la pattern rule li costruisca**

Run:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && make user-bin/gzip.wasm user-bin/gunzip.wasm user-bin/zcat.wasm'
```
Expected: tre `.wasm` copiati in `user-bin/`.

- [ ] **Step 3: Commit** (solo se autorizzato)

```bash
git add Makefile
git commit -m "build: add gzip/gunzip/zcat to BIN_TOOLS"
```

---

### Task 8: Smoke test in-boot

**Files:**
- Modify: `user-bin/smoke.sh` (append in fondo), `Makefile` (assert nel target `run-test`, blocco ~riga 267-291)

- [ ] **Step 1: Appendi la sezione gzip a `user-bin/smoke.sh`** (dopo la riga `ls /bin | wc -l`):

```sh
echo --- gzip smoke ---
cp /etc/init.sh /tmp/gz.txt
gzip /tmp/gz.txt
ls /tmp
zcat /tmp/gz.txt.gz | wc -l
gunzip /tmp/gz.txt.gz
grep -n smoke /tmp/gz.txt
rm /tmp/gz.txt
```

Cosa esercita: gzip file-mode (crea `gz.txt.gz`, cancella `gz.txt`), zcat su pipe, gunzip roundtrip, contenuto ripristinato (il `grep -n` stampa `1:# ruos boot smoke — ...` SOLO se il roundtrip ha restituito il file integro — `/etc/init.sh` in run-test È smoke.sh).

- [ ] **Step 2: Aggiungi l'assert al target `run-test` del Makefile.** Dopo la riga:

```make
	grep -qF "hello from disk" build/serial.log || { echo TEST_FAIL_FAT_CAT; exit 1; }; \
```

aggiungi:

```make
	grep -qF "1:# ruos boot smoke" build/serial.log || { echo TEST_FAIL_GZIP; exit 1; }; \
```

- [ ] **Step 3: Gate completo**

Run:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && make run-test'
```
Expected: `TEST_PASS` (≤240 s di QEMU). Se `TEST_FAIL_GZIP`: cerca in `build/serial.log` le righe dopo `--- gzip smoke ---` per il messaggio d'errore del tool.

- [ ] **Step 4: Commit** (solo se autorizzato)

```bash
git add user-bin/smoke.sh Makefile
git commit -m "test: gzip roundtrip in boot smoke + run-test assert"
```

---

### Task 9: Changelog

**Files:**
- Create: `CHANGELOG/4NN-26-06-10-gzip-tools.md` (NN = numero più alto in `CHANGELOG/` + 1; al momento della stesura il piano è la 402 → implementazione presumibilmente 403, MA va ricontrollato: `ls CHANGELOG | sort | tail`)

- [ ] **Step 1: Crea l'entry**

```markdown
# 4NN — Tool gzip/gunzip/zcat

**Data:** 2026-06-10

## Cosa

Tre tool userland di compressione (wasm32-wasip1, wasmi): `gzip`, `gunzip`,
`zcat`. Formato gzip RFC 1952 via miniz_oxide (pure Rust), lib condivisa
`user/gzip-core` (CRC32, header/trailer, compress/decompress, CLI), semantica
Unix classica (`gzip f` → `f.gz` + delete; flag -c -k -d -1..-9; stdin→stdout
senza argomenti). Smoke roundtrip in `smoke.sh` + assert in `make run-test`.

## Perché

ruos non aveva alcun sistema di compressione (spec
docs/superpowers/specs/2026-06-10-gzip-tools-design.md, piano
docs/superpowers/plans/2026-06-10-gzip-tools.md).

## File toccati

- user/gzip-core/* (nuova lib)
- user/gzip/*, user/gunzip/*, user/zcat/* (nuovi bin)
- user/Cargo.toml, user/Cargo.lock
- Makefile (BIN_TOOLS + assert run-test)
- user-bin/smoke.sh
```

- [ ] **Step 2: Commit** (solo se autorizzato)

```bash
git add CHANGELOG/
git commit -m "chore(changelog): 4NN — gzip/gunzip/zcat tools"
```

---

## Verifica finale (verification-before-completion)

- [ ] `cargo test -p gzip-core` → tutti verdi
- [ ] `make run-test` via WSL → `TEST_PASS` con l'assert gzip attivo
- [ ] Nessuna host fn nuova → `docs/api/` invariato (consapevole, non dimenticato)
