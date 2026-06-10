//! Formato gzip RFC 1952: header (10 byte + campi opzionali FLG), payload
//! deflate raw, trailer CRC32+ISIZE little-endian.

use core::fmt;

use alloc::boxed::Box;
use alloc::vec::Vec;

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

/// Decomprimi un singolo member gzip. Errore se header/trailer invalidi,
/// CRC/ISIZE non tornano, o restano byte dopo il trailer (multi-member non
/// supportato → TrailingGarbage).
pub fn decompress(data: &[u8]) -> Result<Vec<u8>, GzError> {
    let hlen = header_len(data)?;
    if data.len() < hlen + 8 {
        return Err(GzError::TruncatedTrailer);
    }
    // ISIZE (dimensione decompressa mod 2^32) dal trailer → hint per
    // pre-allocare l'output esatto, evitando il raddoppio del buffer.
    let isize_hint = u32::from_le_bytes(data[data.len() - 4..].try_into().unwrap()) as usize;
    let (out, consumed) = inflate_raw(&data[hlen..], isize_hint)?;
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

/// Lunghezza header: 10 byte fissi + campi opzionali da FLG (FEXTRA, FNAME,
/// FCOMMENT, FHCRC) da saltare.
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
    let mut pos = 10usize;
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

/// Inflate raw che riporta anche i byte consumati (serve per localizzare il
/// trailer e rifiutare garbage dopo di esso). `size_hint` = ISIZE del trailer
/// gzip: dimensiona il buffer output ESATTO in un colpo, niente raddoppio.
fn inflate_raw(input: &[u8], size_hint: usize) -> Result<(Vec<u8>, usize), GzError> {
    use miniz_oxide::inflate::core::{decompress as tinfl, inflate_flags, DecompressorOxide};
    use miniz_oxide::inflate::TINFLStatus;

    const FLAGS: u32 = inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF
        | inflate_flags::TINFL_FLAG_IGNORE_ADLER32;
    // Crescita additiva se l'hint è troppo piccolo (ISIZE corrotto): niente
    // raddoppio del buffer, che allocava ~8x l'input → OOM su membri grossi.
    const GROW: usize = 256 * 1024;

    let mut decomp = Box::<DecompressorOxide>::default();
    // Pre-dimensiona all'ISIZE (esatto per output < 4 GiB), limitato a 64x
    // l'input come guardia contro un ISIZE corrotto/gigante.
    let cap = size_hint.min(input.len().saturating_mul(64)).max(256);
    let mut out: Vec<u8> = alloc::vec![0u8; cap];
    let mut total_in = 0usize;
    let mut out_len = 0usize;

    loop {
        let (status, in_consumed, out_consumed) =
            tinfl(&mut decomp, &input[total_in..], &mut out, out_len, FLAGS);

        total_in += in_consumed;
        out_len += out_consumed;

        match status {
            TINFLStatus::Done => {
                out.truncate(out_len);
                return Ok((out, total_in));
            }
            TINFLStatus::HasMoreOutput => {
                // Hint insufficiente: estendi (preserva i byte già scritti,
                // serv­ono al decompressore per le back-reference).
                out.resize(out.len() + GROW, 0u8);
            }
            _ => {
                return Err(GzError::BadDeflate);
            }
        }
    }
}

/// Comprimi `data` in un member gzip completo. `level` 1..=9 (clamp), 6 = default.
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
        // `printf 'hello ruos' | gzip -n -6` su Ubuntu (Step 1). Byte reali:
        const GZ: &[u8] = &[
            0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03,
            0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x57, 0x28, 0x2a, 0xcd, 0x2f,
            0x06, 0x00, 0xb5, 0x7c, 0x1a, 0x7c, 0x0a, 0x00, 0x00, 0x00,
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
}
