//! GPT (GUID Partition Table) reader + writer. M1 parses the primary header at
//! LBA 1 + the partition entry array (read-only). M2a adds `write_layout`, which
//! authors a full GPT (protective MBR + primary/backup headers + entry arrays)
//! whose CRC32s are accepted by real tools (`sgdisk -v`) and by `parse` below.

use crate::blockdev::BlockDevice;
use crate::crc32::crc32;
use alloc::vec::Vec;

/// A contiguous run of sectors on the disk (a partition's placement).
pub struct Extent {
    pub first_lba: u64,
    pub sectors: u64,
}

/// Why `write_layout` could not author the disk.
#[derive(Debug)]
pub enum GptError {
    /// The device is too small (or sized oddly) to hold the requested layout.
    TooSmall,
    /// A `BlockDevice` read/write failed, or the device is not 512-byte sectors.
    Io,
}

/// On-disk type-GUID byte layout (mixed-endian, as GPT stores it).
pub const TYPE_ESP: [u8; 16] =
    [0x28,0x73,0x2A,0xC1, 0x1F,0xF8, 0xD2,0x11, 0xBA,0x4B, 0x00,0xA0,0xC9,0x3E,0xC9,0x3B];
pub const TYPE_MS_BASIC_DATA: [u8; 16] =
    [0xA2,0xA0,0xD0,0xEB, 0xE5,0xB9, 0x33,0x44, 0x87,0xC0, 0x68,0xB6,0xB7,0x26,0x99,0xC7];

#[derive(Clone)]
pub struct GptPartition {
    pub type_guid: [u8; 16],
    pub first_lba: u64,
    pub last_lba: u64,
}
impl GptPartition {
    pub fn is_esp(&self) -> bool { self.type_guid == TYPE_ESP }
    pub fn is_basic_data(&self) -> bool { self.type_guid == TYPE_MS_BASIC_DATA }
    pub fn sectors(&self) -> u64 { self.last_lba.saturating_sub(self.first_lba).saturating_add(1) }
}

fn rd_u32(b: &[u8], o: usize) -> u32 { u32::from_le_bytes([b[o],b[o+1],b[o+2],b[o+3]]) }
fn rd_u64(b: &[u8], o: usize) -> u64 {
    let mut a=[0u8;8]; a.copy_from_slice(&b[o..o+8]); u64::from_le_bytes(a)
}

/// Parse the primary GPT. Returns the non-empty partitions, or None if LBA 1
/// is not a GPT header (caller falls back to a raw FAT at LBA 0).
pub fn parse(dev: &mut dyn BlockDevice) -> Option<Vec<GptPartition>> {
    if dev.block_size() != 512 { return None; }
    let mut hdr = [0u8; 512];
    dev.read_blocks(1, &mut hdr).ok()?;
    if &hdr[0..8] != b"EFI PART" { return None; }

    // Validate the header CRC32. The spec computes it over `header_size` bytes
    // with the header_crc32 field (bytes 16..20) zeroed. Use the header's *own*
    // header_size (clamped to a sane [92, 512] window) — a fixed 92 would reject
    // valid disks whose header_size differs, and >512 would read past the sector.
    let hdr_size = (rd_u32(&hdr, 12) as usize).clamp(92, 512);
    let stored_hdr_crc = rd_u32(&hdr, 16);
    let mut hcopy = [0u8; 512];
    hcopy.copy_from_slice(&hdr);
    hcopy[16..20].copy_from_slice(&[0u8; 4]);
    if crc32(&hcopy[..hdr_size]) != stored_hdr_crc { return None; }

    let entries_lba = rd_u64(&hdr, 72);
    let num = rd_u32(&hdr, 80).min(128) as usize;     // cap at 128 (sane GPT)
    let esize = rd_u32(&hdr, 84) as usize;
    let stored_arr_crc = rd_u32(&hdr, 88);
    // GPT entries are 128 bytes; reject any other size (a non-512-divisor esize
    // would mis-parse entries that straddle a 512-byte sector boundary).
    if esize != 128 || num == 0 { return None; }
    let per_sec = 512 / esize;
    let mut out = Vec::new();
    // Collect the raw entry-array bytes so the array CRC is computed over exactly
    // num*esize bytes (NOT the sector-padded read length — sgdisk's array_crc is
    // over the logical array only).
    let mut arr = Vec::with_capacity(num * esize);
    let mut sec = [0u8; 512];
    let sectors_needed = (num + per_sec - 1) / per_sec;
    for s in 0..sectors_needed {
        let Some(lba) = entries_lba.checked_add(s as u64) else { return None };
        // Retry a marginal sector a couple of times before giving up: a single
        // transient read error on the 32-sector array would otherwise drop the
        // whole mount for this boot (real-hardware robustness). A valid disk
        // succeeds on the first try, so the happy path is unchanged.
        let mut tries = 0u8;
        loop {
            match dev.read_blocks(lba, &mut sec) {
                Ok(())               => break,
                Err(_) if tries < 2  => tries += 1,
                Err(_)               => return None,
            }
        }
        for i in 0..per_sec {
            let idx = s * per_sec + i;
            if idx >= num { break; }
            let e = &sec[i*esize .. i*esize + esize];
            arr.extend_from_slice(e);
            let mut tg = [0u8;16]; tg.copy_from_slice(&e[0..16]);
            if tg == [0u8;16] { continue; } // empty entry
            out.push(GptPartition { type_guid: tg, first_lba: rd_u64(e,32), last_lba: rd_u64(e,40) });
        }
    }
    // Validate the partition-array CRC32 over the logical num*esize bytes.
    if crc32(&arr) != stored_arr_crc { return None; }
    if out.is_empty() { None } else { Some(out) }
}

/// First Microsoft-Basic-Data partition (ruos data partition), if any.
pub fn find_data(parts: &[GptPartition]) -> Option<&GptPartition> {
    parts.iter().find(|p| p.is_basic_data())
}

/// Round `x` up to the next multiple of `a` (`a` a power of two, non-zero),
/// checked — returns None on overflow rather than panicking.
fn align_up(x: u64, a: u64) -> Option<u64> {
    x.checked_add(a - 1).map(|v| (v / a) * a)
}

/// Fill `buf` with fresh GUID bytes from the in-tree CSPRNG (`crate::rng`,
/// ChaCha20 seeded from RDRAND at boot). Validity (CRCs) is the hard requirement;
/// these bytes only need to make the disk_guid / unique_guids distinct.
fn fresh_guid(buf: &mut [u8; 16]) {
    crate::rng::fill(buf);
}

/// Write a UTF-16LE partition name into `dst` (72 bytes, 36 UTF-16 code units),
/// zero-padded. ASCII-only names (we only use "EFI System" / "ruos-data").
fn write_name_utf16le(dst: &mut [u8], name: &str) {
    for (i, ch) in name.chars().enumerate() {
        if i >= 36 { break; }
        let u = ch as u32;
        // All our names are BMP ASCII; encode as a single UTF-16 unit.
        let unit = if u <= 0xFFFF { u as u16 } else { 0xFFFD };
        dst[i*2..i*2+2].copy_from_slice(&unit.to_le_bytes());
    }
}

/// Author a fresh GPT on `dev`: protective MBR (LBA 0), a 128-entry partition
/// array + primary header (LBAs 2..34 and LBA 1), and the backup array + header
/// at the tail. Two partitions are created: an ESP of `esp_sectors` (1 MiB
/// aligned at LBA 2048) and a Microsoft-Basic-Data ("ruos-data") partition that
/// fills the rest of the usable space. All CRC32s are computed so the result is
/// accepted by `sgdisk -v` and by `parse` above. **Destructive** — overwrites
/// the first/last sectors of the device.
///
/// Returns the placement of the (ESP, data) partitions. All layout arithmetic is
/// checked/saturating: an odd or too-small device yields `Err(TooSmall)` rather
/// than a panic (untrusted-disk-safe, like the M1 read path). Requires the
/// CSPRNG (`crate::rng`) to be seeded for the partition GUIDs — guaranteed at the
/// runtime call site (rng is seeded in the userland boot phase, before the shell
/// from which `mkdisk` runs).
pub fn write_layout(
    dev: &mut dyn BlockDevice,
    esp_sectors: u64,
) -> Result<(Extent, Extent), GptError> {
    if dev.block_size() != 512 { return Err(GptError::Io); }
    let total = dev.block_count(); // sectors

    let entries_sectors: u64 = 32; // 128 entries * 128 B / 512
    // Need room for: MBR(1) + primary hdr(1) + primary array(32) + backup
    // array(32) + backup hdr(1), plus at least one usable sector. Guard the tail
    // math against underflow on tiny/odd devices before any subtraction.
    if total < 2 + entries_sectors + entries_sectors + 1 + 1 {
        return Err(GptError::TooSmall);
    }
    let first_usable = 2 + entries_sectors; // LBA 34
    let backup_hdr_lba = total - 1;
    let backup_arr_lba = backup_hdr_lba - entries_sectors; // 32 sectors before
    let last_usable = backup_arr_lba - 1;

    let esp_first: u64 = 2048; // 1 MiB align
    let esp_last = esp_first
        .checked_add(esp_sectors)
        .and_then(|v| v.checked_sub(1))
        .ok_or(GptError::TooSmall)?;
    let data_first = align_up(esp_last.checked_add(1).ok_or(GptError::TooSmall)?, 2048)
        .ok_or(GptError::TooSmall)?;
    let data_last = last_usable;

    // Validate placement: ESP starts in usable space, ESP ends before data
    // starts, and data has at least one sector.
    if !(esp_first >= first_usable && esp_last < data_first && data_first <= data_last) {
        return Err(GptError::TooSmall);
    }

    // --- 1. Protective MBR (LBA 0) ---------------------------------------
    let mut mbr = [0u8; 512];
    // One partition record at offset 0x1BE covering the whole disk as 0xEE.
    let r = 0x1BE;
    mbr[r] = 0x00;                       // boot indicator
    mbr[r + 1] = 0x00;                   // start CHS (head)
    mbr[r + 2] = 0x02;                   // start CHS (sector=2)
    mbr[r + 3] = 0x00;                   // start CHS (cylinder)
    mbr[r + 4] = 0xEE;                   // OS type = GPT protective
    mbr[r + 5] = 0xFF;                   // end CHS (head)
    mbr[r + 6] = 0xFF;                   // end CHS (sector)
    mbr[r + 7] = 0xFF;                   // end CHS (cylinder)
    mbr[r + 8..r + 12].copy_from_slice(&1u32.to_le_bytes()); // starting LBA = 1
    let mbr_size = core::cmp::min(total.saturating_sub(1), 0xFFFF_FFFF) as u32;
    mbr[r + 12..r + 16].copy_from_slice(&mbr_size.to_le_bytes()); // size in LBA
    mbr[510] = 0x55;
    mbr[511] = 0xAA;

    // --- 2. Partition entry array (32 sectors = 16384 bytes) -------------
    let mut arr = [0u8; 16384];
    // entry 0 = ESP
    {
        let e = &mut arr[0..128];
        e[0..16].copy_from_slice(&TYPE_ESP);
        let mut ug = [0u8; 16]; fresh_guid(&mut ug);
        e[16..32].copy_from_slice(&ug);
        e[32..40].copy_from_slice(&esp_first.to_le_bytes());
        e[40..48].copy_from_slice(&esp_last.to_le_bytes());
        // attributes [48..56] = 0
        write_name_utf16le(&mut e[56..128], "EFI System");
    }
    // entry 1 = Microsoft Basic Data ("ruos-data")
    {
        let e = &mut arr[128..256];
        e[0..16].copy_from_slice(&TYPE_MS_BASIC_DATA);
        let mut ug = [0u8; 16]; fresh_guid(&mut ug);
        e[16..32].copy_from_slice(&ug);
        e[32..40].copy_from_slice(&data_first.to_le_bytes());
        e[40..48].copy_from_slice(&data_last.to_le_bytes());
        write_name_utf16le(&mut e[56..128], "ruos-data");
    }
    // entries 2..128 already zero.
    let array_crc = crc32(&arr[..]);

    // --- 3. Primary header (LBA 1) --------------------------------------
    let mut disk_guid = [0u8; 16];
    fresh_guid(&mut disk_guid);
    let primary = build_header(
        /* my_lba       */ 1,
        /* alternate    */ backup_hdr_lba,
        first_usable,
        last_usable,
        &disk_guid,
        /* entry_lba    */ 2,
        array_crc,
    );

    // --- 4. Backup header (LBA = backup_hdr_lba) ------------------------
    let backup = build_header(
        /* my_lba       */ backup_hdr_lba,
        /* alternate    */ 1,
        first_usable,
        last_usable,
        &disk_guid,
        /* entry_lba    */ backup_arr_lba,
        array_crc,
    );

    // --- 5. Write order: array, primary hdr, MBR, backup array, backup hdr
    let w = |dev: &mut dyn BlockDevice, lba: u64, buf: &[u8]| -> Result<(), GptError> {
        dev.write_blocks(lba, buf).map_err(|_| GptError::Io)
    };
    w(dev, 2, &arr)?;                  // primary array @ LBA 2 (32 sectors)
    w(dev, 1, &primary)?;             // primary header @ LBA 1
    w(dev, 0, &mbr)?;                 // protective MBR @ LBA 0
    w(dev, backup_arr_lba, &arr)?;   // backup array (32 sectors)
    w(dev, backup_hdr_lba, &backup)?; // backup header

    Ok((
        Extent { first_lba: esp_first, sectors: esp_sectors },
        Extent { first_lba: data_first, sectors: data_last - data_first + 1 },
    ))
}

/// Build a 512-byte GPT header sector (92-byte header in the first bytes, rest
/// zero) and compute its header_crc32 (field zeroed, CRC over the first 92 bytes).
fn build_header(
    my_lba: u64,
    alternate_lba: u64,
    first_usable: u64,
    last_usable: u64,
    disk_guid: &[u8; 16],
    partition_entry_lba: u64,
    array_crc: u32,
) -> [u8; 512] {
    let mut h = [0u8; 512];
    h[0..8].copy_from_slice(b"EFI PART");
    h[8..12].copy_from_slice(&[0x00, 0x00, 0x01, 0x00]); // revision 1.0
    h[12..16].copy_from_slice(&92u32.to_le_bytes());     // header_size
    // h[16..20] header_crc32 left zero for now
    // h[20..24] reserved = 0
    h[24..32].copy_from_slice(&my_lba.to_le_bytes());
    h[32..40].copy_from_slice(&alternate_lba.to_le_bytes());
    h[40..48].copy_from_slice(&first_usable.to_le_bytes());
    h[48..56].copy_from_slice(&last_usable.to_le_bytes());
    h[56..72].copy_from_slice(disk_guid);
    h[72..80].copy_from_slice(&partition_entry_lba.to_le_bytes());
    h[80..84].copy_from_slice(&128u32.to_le_bytes());    // num_partition_entries
    h[84..88].copy_from_slice(&128u32.to_le_bytes());    // size_of_partition_entry
    h[88..92].copy_from_slice(&array_crc.to_le_bytes());
    let hdr_crc = crc32(&h[0..92]);
    h[16..20].copy_from_slice(&hdr_crc.to_le_bytes());
    h
}

#[cfg(test)]
mod tests {
    use super::*; extern crate std; use std::vec; use std::vec::Vec as SVec;

    struct MemDev(SVec<u8>);
    impl crate::blockdev::BlockDevice for MemDev {
        fn block_size(&self)->u32 {512}
        fn block_count(&self)->u64 {(self.0.len()/512) as u64}
        fn read_blocks(&mut self,lba:u64,buf:&mut[u8])->Result<(),crate::blockdev::BlockError>{
            let o=(lba as usize)*512;
            if o+buf.len()>self.0.len(){return Err(crate::blockdev::BlockError::OutOfRange);}
            buf.copy_from_slice(&self.0[o..o+buf.len()]); Ok(())
        }
        fn write_blocks(&mut self,lba:u64,buf:&[u8])->Result<(),crate::blockdev::BlockError>{
            let o=(lba as usize)*512; self.0[o..o+buf.len()].copy_from_slice(buf); Ok(())
        }
    }

    // A hand-built primary GPT with VALID CRC32s (header + array), so it passes
    // the M2a read-side CRC check. The entry array (num*esize = 16384 bytes)
    // lives at LBA 2 (offsets 1024..17408), within this 40-sector device.
    fn synth() -> MemDev {
        let mut d = vec![0u8; 512*40];
        d[512..520].copy_from_slice(b"EFI PART");
        d[512+12..512+16].copy_from_slice(&92u32.to_le_bytes()); // header_size
        d[512+72..512+80].copy_from_slice(&2u64.to_le_bytes());
        d[512+80..512+84].copy_from_slice(&128u32.to_le_bytes());
        d[512+84..512+88].copy_from_slice(&128u32.to_le_bytes());
        let e0 = 2*512;
        d[e0..e0+16].copy_from_slice(&TYPE_ESP);
        d[e0+32..e0+40].copy_from_slice(&34u64.to_le_bytes());
        d[e0+40..e0+48].copy_from_slice(&2047u64.to_le_bytes());
        let e1 = e0+128;
        d[e1..e1+16].copy_from_slice(&TYPE_MS_BASIC_DATA);
        d[e1+32..e1+40].copy_from_slice(&2048u64.to_le_bytes());
        d[e1+40..e1+48].copy_from_slice(&4095u64.to_le_bytes());
        // partition_array_crc32 over the logical 16384-byte array.
        let arr_crc = crc32(&d[e0..e0+16384]);
        d[512+88..512+92].copy_from_slice(&arr_crc.to_le_bytes());
        // header_crc32 over the first 92 bytes with the crc field zeroed.
        d[512+16..512+20].copy_from_slice(&[0u8;4]);
        let hdr_crc = crc32(&d[512..512+92]);
        d[512+16..512+20].copy_from_slice(&hdr_crc.to_le_bytes());
        MemDev(d)
    }

    #[test] fn parses_two_parts() {
        let mut d = synth();
        let p = parse(&mut d).unwrap();
        assert_eq!(p.len(), 2);
        assert!(p[0].is_esp());
        assert!(p[1].is_basic_data());
        assert_eq!(p[1].first_lba, 2048);
        assert_eq!(p[1].sectors(), 2048);
    }
    #[test] fn no_gpt_is_none() {
        let mut d = MemDev(vec![0u8; 512*4]);
        assert!(parse(&mut d).is_none());
    }
    #[test] fn find_data_picks_basic() {
        let mut d = synth();
        let p = parse(&mut d).unwrap();
        let data = find_data(&p).unwrap();
        assert_eq!(data.first_lba, 2048);
    }
    #[test] fn bad_header_crc_is_none() {
        // Corrupt one byte in the synth header (after the crc field) → CRC fails.
        let mut d = synth();
        d.0[512 + 40] ^= 0xFF; // first_usable_lba byte, covered by header CRC
        assert!(parse(&mut d).is_none());
    }

    // --- M2a write_layout round-trip --------------------------------------

    #[test] fn write_layout_roundtrip() {
        // 256 MiB device, 64 MiB ESP.
        let mut d = MemDev(vec![0u8; 512 * 524288]);
        let (esp, data) = write_layout(&mut d, 131072).expect("write_layout ok");
        assert_eq!(esp.first_lba, 2048);
        assert_eq!(esp.sectors, 131072);

        // The new CRC-checked reader must accept what the writer produced.
        let p = parse(&mut d).expect("parse ok");
        assert_eq!(p.len(), 2);
        assert!(p[0].is_esp());
        assert!(p[1].is_basic_data());
        // Data partition extent reported by the writer matches what parse reads.
        assert_eq!(p[1].first_lba, data.first_lba);
        assert_eq!(p[1].sectors(), data.sectors);

        // Protective MBR sanity.
        assert_eq!(d.0[0x1BE + 4], 0xEE);
        assert_eq!(d.0[510], 0x55);
        assert_eq!(d.0[511], 0xAA);
    }

    #[test] fn flip_primary_header_byte_rejected() {
        let mut d = MemDev(vec![0u8; 512 * 524288]);
        write_layout(&mut d, 131072).expect("write_layout ok");
        assert!(parse(&mut d).is_some());
        // Flip a byte in the primary header sector (LBA 1) → CRC catches it.
        d.0[512 + 24] ^= 0x01; // my_lba byte, covered by header CRC
        assert!(parse(&mut d).is_none());
    }

    #[test] fn too_small_device_errs() {
        let mut d = MemDev(vec![0u8; 512 * 100]); // far too small for esp_first=2048
        assert!(write_layout(&mut d, 10).is_err());
    }
}
