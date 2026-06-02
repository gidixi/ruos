//! Minimal GPT (GUID Partition Table) reader. Parses the primary header at
//! LBA 1 + the partition entry array. Read-only (M1); writing is M2.

use crate::blockdev::BlockDevice;
use alloc::vec::Vec;

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
    pub fn sectors(&self) -> u64 { self.last_lba.saturating_sub(self.first_lba) + 1 }
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
    let entries_lba = rd_u64(&hdr, 72);
    let num = rd_u32(&hdr, 80).min(128) as usize;     // cap at 128 (sane GPT)
    let esize = rd_u32(&hdr, 84) as usize;
    if !(128..=512).contains(&esize) || num == 0 { return None; }
    let per_sec = 512 / esize;
    if per_sec == 0 { return None; }
    let mut out = Vec::new();
    let mut sec = [0u8; 512];
    let sectors_needed = (num + per_sec - 1) / per_sec;
    for s in 0..sectors_needed {
        if dev.read_blocks(entries_lba + s as u64, &mut sec).is_err() { break; }
        for i in 0..per_sec {
            let idx = s * per_sec + i;
            if idx >= num { break; }
            let e = &sec[i*esize .. i*esize + esize];
            let mut tg = [0u8;16]; tg.copy_from_slice(&e[0..16]);
            if tg == [0u8;16] { continue; } // empty entry
            out.push(GptPartition { type_guid: tg, first_lba: rd_u64(e,32), last_lba: rd_u64(e,40) });
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// First Microsoft-Basic-Data partition (ruos data partition), if any.
pub fn find_data(parts: &[GptPartition]) -> Option<&GptPartition> {
    parts.iter().find(|p| p.is_basic_data())
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

    fn synth() -> MemDev {
        let mut d = vec![0u8; 512*40];
        d[512..520].copy_from_slice(b"EFI PART");
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
}
