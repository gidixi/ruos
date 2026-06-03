//! Reflected CRC-32 (IEEE 802.3 / zlib / GPT). Init 0xFFFFFFFF, input+output
//! reflected, final XOR 0xFFFFFFFF.

/// CRC-32 of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn known_vector() { assert_eq!(crc32(b"123456789"), 0xCBF4_3926); }
    #[test] fn empty() { assert_eq!(crc32(b""), 0x0000_0000); }
    #[test] fn one_byte() { assert_eq!(crc32(&[0x00]), 0xD202_EF8D); }
}
