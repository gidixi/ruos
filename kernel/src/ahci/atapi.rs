//! SCSI MMC (ATAPI) command descriptor blocks — funzioni pure.
//!
//! Usati da `AhciPort::issue_atapi` per leggere un CD-ROM via PACKET.
//! Tenuti puri (no MMIO) così sono unit-testabili su host.

/// SCSI READ(10) — opcode 0x28. `lba` in blocchi logici (2048 B su CD),
/// `count` = numero di blocchi da leggere. CDB a 12 byte (campo ATAPI ACMD).
pub fn read10_cdb(lba: u32, count: u16) -> [u8; 12] {
    let mut c = [0u8; 12];
    c[0] = 0x28;
    c[2] = (lba >> 24) as u8;
    c[3] = (lba >> 16) as u8;
    c[4] = (lba >> 8) as u8;
    c[5] = lba as u8;
    c[7] = (count >> 8) as u8;
    c[8] = count as u8;
    c
}

/// SCSI READ CAPACITY(10) — opcode 0x25. Ritorna 8 byte: last LBA (BE) + block size (BE).
pub fn read_capacity10_cdb() -> [u8; 12] {
    let mut c = [0u8; 12];
    c[0] = 0x25;
    c
}

/// Parsa la risposta di READ CAPACITY(10): `(last_lba, block_size)`.
pub fn parse_read_capacity10(buf: &[u8]) -> Option<(u32, u32)> {
    if buf.len() < 8 { return None; }
    let last = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let bs   = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
    Some((last, bs))
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;

    #[test] fn read10_encodes_lba_and_count() {
        let cdb = read10_cdb(0x0001_0203, 0x0405);
        assert_eq!(cdb[0], 0x28);
        assert_eq!(&cdb[2..6], &[0x00, 0x01, 0x02, 0x03]);
        assert_eq!(&cdb[7..9], &[0x04, 0x05]);
    }

    #[test] fn capacity_roundtrip() {
        let resp = [0x00, 0x0F, 0xFF, 0xFF, 0x00, 0x00, 0x08, 0x00];
        assert_eq!(parse_read_capacity10(&resp), Some((0x000F_FFFF, 2048)));
    }

    #[test] fn capacity_short_buf_none() {
        assert_eq!(parse_read_capacity10(&[0u8; 4]), None);
    }
}
