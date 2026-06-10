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
    // Guard: numero entry deve stare in u32.
    debug_assert!(entries.len() <= u32::MAX as usize, "troppe entry");
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (name, data) in entries {
        let nb = name.as_bytes();
        // Guard: il nome deve stare in u16.
        debug_assert!(nb.len() <= u16::MAX as usize, "nome entry troppo lungo");
        out.extend_from_slice(&(nb.len() as u16).to_le_bytes());
        out.extend_from_slice(nb);
        let gz = compress(data, level);
        // Guard: il membro gzip deve stare in u32.
        debug_assert!(gz.len() <= u32::MAX as usize, "membro gzip troppo grande");
        out.extend_from_slice(&(gz.len() as u32).to_le_bytes());
        out.extend_from_slice(&gz);
    }
    out
}

/// Iteratore sulle entry: `(name, gz_member)` senza decomprimere.
#[derive(Debug)]
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
        // Controllo overflow-safe: 2 byte per name_len.
        if p.saturating_add(2) > d.len() {
            // Avvelena l'iteratore: le chiamate successive restituiranno None.
            self.remaining = 0;
            return Some(Err(PackError::Truncated));
        }
        let nlen = u16::from_le_bytes([d[p], d[p + 1]]) as usize;
        p += 2;
        // Controllo overflow-safe: name + 4 byte per gz_len.
        if p.saturating_add(nlen).saturating_add(4) > d.len() {
            self.remaining = 0;
            return Some(Err(PackError::Truncated));
        }
        let name = match core::str::from_utf8(&d[p..p + nlen]) {
            Ok(s) => s,
            Err(_) => {
                // Nome non UTF-8 → entry malformata, avvelena l'iteratore.
                self.remaining = 0;
                return Some(Err(PackError::Truncated));
            }
        };
        p += nlen;
        let glen = u32::from_le_bytes(d[p..p + 4].try_into().unwrap()) as usize;
        p += 4;
        // Controllo overflow-safe: dati gzip.
        if p.saturating_add(glen) > d.len() {
            self.remaining = 0;
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut a = Vec::new();
        a.extend_from_slice(b"RBIN");
        a.push(1u8);
        a.extend_from_slice(&1u32.to_le_bytes());
        let err = parse(&a).unwrap().next().unwrap().unwrap_err();
        assert_eq!(err, PackError::Truncated);
    }
}
