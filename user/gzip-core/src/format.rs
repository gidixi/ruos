//! Formato gzip RFC 1952: header (10 byte + campi opzionali FLG), payload
//! deflate raw, trailer CRC32+ISIZE little-endian.

use core::fmt;

use crate::crc32::crc32;

pub const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];
const CM_DEFLATE: u8 = 8;
#[allow(dead_code)]
const FHCRC: u8 = 2;
#[allow(dead_code)]
const FEXTRA: u8 = 4;
#[allow(dead_code)]
const FNAME: u8 = 8;
#[allow(dead_code)]
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
}
