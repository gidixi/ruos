//! Per-port AHCI engine + ATA command issuing.
//!
//! Each port owns a `DmaRegion` (2 pages = 8 KiB) split into:
//! - `+0x000` Command List (32 × 32-byte Command Headers = 1024 B)
//! - `+0x400` Received FIS area (256 B)
//! - `+0x500` Command Table 0 (Command FIS 64 B + scratch + PRDT[0])
//! - `+0x600` Scratch sector buffer (512 B) for IDENTIFY DEVICE
//!
//! Slot 0 only — polled, single-outstanding-command. NCQ later.

use alloc::string::String;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};

use x86_64::VirtAddr;

use crate::blockdev::{BlockDevice, BlockError};
use crate::memory::dma::{alloc as dma_alloc, DmaRegion};

// Per-port register offsets from ABAR + 0x100 + port_idx * 0x80.
const PXCLB:  usize = 0x00;
const PXCLBU: usize = 0x04;
const PXFB:   usize = 0x08;
const PXFBU:  usize = 0x0C;
const PXIS:   usize = 0x10;
const PXIE:   usize = 0x14;
const PXCMD:  usize = 0x18;
const PXTFD:  usize = 0x20;
const PXSIG:  usize = 0x24;
const PXSSTS: usize = 0x28;
const PXSERR: usize = 0x30;
const PXCI:   usize = 0x38;

// PxCMD bits
const PXCMD_ST:  u32 = 1 << 0;
const PXCMD_FRE: u32 = 1 << 4;
const PXCMD_FR:  u32 = 1 << 14;
const PXCMD_CR:  u32 = 1 << 15;

// PxTFD bits
const TFD_BSY: u32 = 1 << 7;
const TFD_DRQ: u32 = 1 << 3;
const TFD_ERR: u32 = 1 << 0;

// SATA disk signature reported in PxSIG.
const SIG_SATA: u32 = 0x0000_0101;

// ATAPI device signature reported in PxSIG (CD-ROM, packet device).
const SIG_ATAPI: u32 = 0xEB14_0101;

// ATA PACKET command + DMA feature bit.
const ATA_PACKET: u8 = 0xA0;
const PACKET_FEATURE_DMA: u8 = 1 << 0;

// Command-header flag bit A (ATAPI) — bit 5 of `flags`.
const CH_FLAG_ATAPI: u16 = 1 << 5;

// CD-ROM logical block size.
const ATAPI_BLOCK: usize = 2048;

// DMA region layout offsets.
const OFF_CL:      usize = 0x000;
const OFF_FIS:     usize = 0x400;
const OFF_CT0:     usize = 0x500;
const OFF_SCRATCH: usize = 0x600;

// Polling timeouts (timer ticks @ 100 Hz).
const STOP_TIMEOUT:  u64 = 100;  // ~1 s for engine stop
const CMD_TIMEOUT:   u64 = 500;  // ~5 s per command

// ATA commands.
const ATA_IDENTIFY:        u8 = 0xEC;
const ATA_READ_DMA_EXT:    u8 = 0x25;
const ATA_WRITE_DMA_EXT:   u8 = 0x35;
const ATA_FLUSH_CACHE_EXT: u8 = 0xEA;

/// 32-byte AHCI Command Header.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct CmdHeader {
    /// Bits: CFL (4:0) | A (5) | W (6) | P (7) | R (8) | B (9) | C (10) | rsv (11) | PMP (15:12)
    flags:    u16,
    prdtl:    u16,
    prdbc:    u32,
    ctba:     u32,
    ctbau:    u32,
    rsv:      [u32; 4],
}

/// Command Table FIS slot (one Command FIS + ATAPI cmd + reserved + PRDT[0]).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct CmdTable {
    cfis:  [u8; 64],
    acmd:  [u8; 16],
    rsv:   [u8; 48],
    prdt0: PrdtEntry,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct PrdtEntry {
    dba:   u32,
    dbau:  u32,
    rsv:   u32,
    /// Bits 0..21 = data byte count - 1; bit 31 = IRQ on completion.
    dbc:   u32,
}

/// Host-to-Device Register FIS.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FisH2D {
    fis_type: u8,  // 0x27
    flags:    u8,  // bit 7 = C (command vs control)
    command:  u8,
    feature_l:u8,

    lba0: u8, lba1: u8, lba2: u8,
    device: u8,

    lba3: u8, lba4: u8, lba5: u8,
    feature_h: u8,

    count_l: u8, count_h: u8,
    icc: u8, control: u8,

    rsv: [u8; 4],
}

const FIS_TYPE_REG_H2D: u8 = 0x27;
const FIS_FLAG_CMD:     u8 = 1 << 7;

pub struct AhciPort {
    abar:      VirtAddr,
    port_idx:  usize,
    dma:       DmaRegion,
    pub sectors: u64,
    pub model:   String,
    pub is_atapi: bool,
}

impl AhciPort {
    /// Bring up port `port_idx`. Returns `None` if no SATA device present or
    /// init failed (logged via bwarn).
    pub fn bringup(abar: VirtAddr, port_idx: usize) -> Option<Self> {
        // 1. Presence: PxSSTS.DET == 3 (device present + PHY ready).
        //    PxSIG is NOT meaningful yet — the device latches it only after
        //    the FIS Receive engine is running and the first D2H Register FIS
        //    arrives. We check DET here, sig after FRE.
        let port_base = port_addr(abar, port_idx);
        let det = read32(port_base + PXSSTS) & 0xF;
        if det != 3 {
            return None;
        }

        // 2. Stop engine: clear ST → wait CR=0; then clear FRE → wait FR=0.
        let cmd = read32(port_base + PXCMD);
        write32(port_base + PXCMD, cmd & !PXCMD_ST);
        if !wait_clear(port_base + PXCMD, PXCMD_CR, STOP_TIMEOUT) {
            crate::bwarn!("ahci", "port {} CR didn't clear", port_idx);
            return None;
        }
        let cmd = read32(port_base + PXCMD);
        write32(port_base + PXCMD, cmd & !PXCMD_FRE);
        if !wait_clear(port_base + PXCMD, PXCMD_FR, STOP_TIMEOUT) {
            crate::bwarn!("ahci", "port {} FR didn't clear", port_idx);
            return None;
        }

        // 3. Allocate 2 contiguous pages: CL + FIS + CT0 + scratch all fit.
        let dma = match dma_alloc(2) {
            Some(r) => r,
            None    => { crate::bwarn!("ahci", "port {} dma alloc fail", port_idx); return None; }
        };

        let fis_phys = dma.phys.as_u64() + OFF_FIS as u64;
        let cl_phys  = dma.phys.as_u64() + OFF_CL  as u64;

        // 4. Program PxCLB/PxFB to the DMA region.
        write32(port_base + PXCLB,  cl_phys as u32);
        write32(port_base + PXCLBU, (cl_phys >> 32) as u32);
        write32(port_base + PXFB,   fis_phys as u32);
        write32(port_base + PXFBU,  (fis_phys >> 32) as u32);

        // 5. Clear any pending SATA errors + interrupt status.
        write32(port_base + PXSERR, 0xFFFF_FFFF);
        write32(port_base + PXIS,   0xFFFF_FFFF);

        // 6. Mask all port-level interrupts (polling).
        write32(port_base + PXIE, 0);

        // 7. Start FIS Receive — the device's D2H Register FIS will populate
        //    PxSIG. Don't set ST yet; commands go after sig is valid.
        let cmd = read32(port_base + PXCMD);
        write32(port_base + PXCMD, cmd | PXCMD_FRE);

        // 8. Wait for BSY/DRQ to settle + a valid signature. QEMU posts the
        //    D2H FIS within ~10 ms; we cap at 1 s.
        let start = crate::timer::ticks();
        loop {
            let tfd = read32(port_base + PXTFD);
            let sig = read32(port_base + PXSIG);
            if (tfd & (TFD_BSY | TFD_DRQ)) == 0 && sig != 0xFFFF_FFFF {
                break;
            }
            if crate::timer::ticks().wrapping_sub(start) > STOP_TIMEOUT {
                crate::bwarn!(
                    "ahci",
                    "port {} sig settle timeout tfd=0x{:08x} sig=0x{:08x}",
                    port_idx, tfd, sig,
                );
                return None;
            }
            core::hint::spin_loop();
        }

        let sig = read32(port_base + PXSIG);
        let is_atapi = match sig {
            SIG_SATA  => false,
            SIG_ATAPI => true,
            other => {
                crate::bwarn!("ahci", "port {} unknown sig=0x{:08x}", port_idx, other);
                return None;
            }
        };

        // 9. Enable command engine.
        let cmd = read32(port_base + PXCMD);
        write32(port_base + PXCMD, cmd | PXCMD_ST);

        let mut p = Self { abar, port_idx, dma, sectors: 0, model: String::new(), is_atapi };

        // 10. ATA IDENTIFY (SATA) o READ CAPACITY (ATAPI) per i settori.
        if p.is_atapi {
            match p.atapi_read_capacity() {
                Ok(sectors) => {
                    p.sectors = sectors;
                    p.model = String::from("ATAPI CD-ROM");
                    crate::binfo!("ahci", "port {} atapi sectors={} (2048B)", port_idx, sectors);
                }
                Err(e) => {
                    crate::bwarn!("ahci", "port {} READ CAPACITY failed: {}", port_idx, e);
                    return None;
                }
            }
        } else {
            if let Err(e) = p.identify() {
                crate::bwarn!("ahci", "port {} IDENTIFY failed: {}", port_idx, e);
                return None;
            }
            crate::binfo!(
                "ahci", "port {} sata sectors={} model=\"{}\"",
                port_idx, p.sectors, p.model.trim()
            );
        }
        // Cache this port's IDENTIFY info so `disks` can report it later WITHOUT
        // re-bringing-up the port — critical for a port that is about to be moved
        // into the `/mnt` mount, where a second bringup would reprogram its live
        // PxCLB/PxFB and corrupt in-flight DMA. Every bringup caches (incl. the
        // boot-time bringup of the mounted port in storage.rs).
        crate::ahci::cache_disk_info(port_idx, p.model.clone(), p.sectors);
        Some(p)
    }

    fn port_base(&self) -> usize { port_addr(self.abar, self.port_idx) }

    fn ct0_virt(&self) -> *mut CmdTable {
        (self.dma.virt.as_u64() as usize + OFF_CT0) as *mut CmdTable
    }
    fn ct0_phys(&self) -> u64 { self.dma.phys.as_u64() + OFF_CT0 as u64 }
    fn ch0_virt(&self) -> *mut CmdHeader {
        (self.dma.virt.as_u64() as usize + OFF_CL) as *mut CmdHeader
    }

    /// Build CT0 + CH0 for a one-PRDT command, fire it, wait completion.
    fn issue(
        &mut self,
        cmd: u8,
        lba: u64,
        sectors: u16,
        buf_phys: u64,
        buf_bytes: u32,
        write: bool,
    ) -> Result<(), BlockError> {
        // Build the Command FIS (H2D Register).
        let fis = FisH2D {
            fis_type:  FIS_TYPE_REG_H2D,
            flags:     FIS_FLAG_CMD,
            command:   cmd,
            feature_l: 0,
            lba0:  (lba       ) as u8,
            lba1:  (lba >>  8 ) as u8,
            lba2:  (lba >> 16 ) as u8,
            device: 0x40,            // LBA mode bit
            lba3:  (lba >> 24 ) as u8,
            lba4:  (lba >> 32 ) as u8,
            lba5:  (lba >> 40 ) as u8,
            feature_h: 0,
            count_l: (sectors      ) as u8,
            count_h: (sectors >>  8) as u8,
            icc: 0,
            control: 0,
            rsv: [0; 4],
        };

        // No-data commands (e.g. FLUSH CACHE EXT) carry no PRDT: PRDTL=0 and the
        // HBA never touches the PRDT region. A non-zero PRDTL with no DMA would
        // hang the engine, and `buf_bytes - 1` would underflow for buf_bytes==0.
        let has_data = buf_bytes > 0;

        // Build CT0: copy CFIS bytes, fill PRDT[0]. Write through the raw
        // pointer with `addr_of_mut!` + `write` to avoid implicit autoref to
        // unaligned/HHDM-backed memory.
        unsafe {
            let ct = self.ct0_virt();
            // Write a fresh CT contents directly: zeroed cfis + acmd + rsv,
            // then memcpy the FIS bytes and assign PRDT.
            let cfis_ptr = core::ptr::addr_of_mut!((*ct).cfis) as *mut u8;
            core::ptr::write_bytes(cfis_ptr, 0, 64);
            let fis_bytes = core::slice::from_raw_parts(
                &fis as *const FisH2D as *const u8,
                core::mem::size_of::<FisH2D>(),
            );
            core::ptr::copy_nonoverlapping(fis_bytes.as_ptr(), cfis_ptr, fis_bytes.len());
            let prdt0_ptr = core::ptr::addr_of_mut!((*ct).prdt0);
            // Only program a PRDT for data-bearing commands; for no-data commands
            // zero it (PRDTL=0 below means the HBA ignores it anyway).
            core::ptr::write(prdt0_ptr, if has_data {
                PrdtEntry {
                    dba:  buf_phys as u32,
                    dbau: (buf_phys >> 32) as u32,
                    rsv:  0,
                    // dbc bits 0..21 = byte count - 1, must be even
                    dbc:  (buf_bytes - 1) & 0x3F_FFFF,
                }
            } else {
                PrdtEntry { dba: 0, dbau: 0, rsv: 0, dbc: 0 }
            });
        }

        // Build Command Header[0]: CFL=5 dwords (one register FIS = 5*4 = 20 B),
        // W=1 for writes, PRDTL=1 for data commands / 0 for no-data, CTBA=CT0 phys.
        let ct0_phys = self.ct0_phys();
        let cfl_dwords = (core::mem::size_of::<FisH2D>() as u16 / 4) & 0x1F;
        let mut flags: u16 = cfl_dwords;
        if write { flags |= 1 << 6; }
        unsafe {
            let ch = self.ch0_virt();
            write_volatile(ch, CmdHeader {
                flags,
                prdtl: if has_data { 1 } else { 0 },
                prdbc: 0,
                ctba:  ct0_phys as u32,
                ctbau: (ct0_phys >> 32) as u32,
                rsv:   [0; 4],
            });
        }

        // Wait for BSY/DRQ to clear before issuing.
        let pb = self.port_base();
        let start = crate::timer::ticks();
        while (read32(pb + PXTFD) & (TFD_BSY | TFD_DRQ)) != 0 {
            if crate::timer::ticks().wrapping_sub(start) > CMD_TIMEOUT {
                return Err(BlockError::Timeout);
            }
            core::hint::spin_loop();
        }

        // Clear PxIS and fire slot 0.
        write32(pb + PXIS, 0xFFFF_FFFF);
        write32(pb + PXCI, 1);

        // Poll for completion.
        let start = crate::timer::ticks();
        while (read32(pb + PXCI) & 1) != 0 {
            if crate::timer::ticks().wrapping_sub(start) > CMD_TIMEOUT {
                return Err(BlockError::Timeout);
            }
            if (read32(pb + PXTFD) & TFD_ERR) != 0 {
                return Err(BlockError::Io);
            }
            core::hint::spin_loop();
        }
        if (read32(pb + PXTFD) & TFD_ERR) != 0 {
            return Err(BlockError::Io);
        }
        Ok(())
    }

    /// Emette un comando ATAPI PACKET (DMA-in) con il CDB SCSI dato. Modellato
    /// su `issue`, ma: command=PACKET, feature=DMA, command-header flag A=1, e il
    /// CDB copiato nel campo `acmd` della Command Table.
    fn issue_atapi(
        &mut self,
        cdb: &[u8; 12],
        buf_phys: u64,
        buf_bytes: u32,
    ) -> Result<(), BlockError> {
        let fis = FisH2D {
            fis_type:  FIS_TYPE_REG_H2D,
            flags:     FIS_FLAG_CMD,
            command:   ATA_PACKET,
            feature_l: PACKET_FEATURE_DMA,
            lba0: 0,
            lba1: (buf_bytes & 0xFF) as u8,
            lba2: (buf_bytes >> 8) as u8,
            device: 0,
            lba3: 0, lba4: 0, lba5: 0, feature_h: 0,
            count_l: 0, count_h: 0, icc: 0, control: 0,
            rsv: [0; 4],
        };

        unsafe {
            let ct = self.ct0_virt();
            let cfis_ptr = core::ptr::addr_of_mut!((*ct).cfis) as *mut u8;
            core::ptr::write_bytes(cfis_ptr, 0, 64);
            let fis_bytes = core::slice::from_raw_parts(
                &fis as *const FisH2D as *const u8,
                core::mem::size_of::<FisH2D>(),
            );
            core::ptr::copy_nonoverlapping(fis_bytes.as_ptr(), cfis_ptr, fis_bytes.len());
            let acmd_ptr = core::ptr::addr_of_mut!((*ct).acmd) as *mut u8;
            core::ptr::write_bytes(acmd_ptr, 0, 16);
            core::ptr::copy_nonoverlapping(cdb.as_ptr(), acmd_ptr, 12);
            let prdt0_ptr = core::ptr::addr_of_mut!((*ct).prdt0);
            core::ptr::write(prdt0_ptr, PrdtEntry {
                dba:  buf_phys as u32,
                dbau: (buf_phys >> 32) as u32,
                rsv:  0,
                dbc:  (buf_bytes - 1) & 0x3F_FFFF,
            });
        }

        let ct0_phys = self.ct0_phys();
        let cfl_dwords = (core::mem::size_of::<FisH2D>() as u16 / 4) & 0x1F;
        let flags: u16 = cfl_dwords | CH_FLAG_ATAPI;
        unsafe {
            let ch = self.ch0_virt();
            write_volatile(ch, CmdHeader {
                flags,
                prdtl: 1,
                prdbc: 0,
                ctba:  ct0_phys as u32,
                ctbau: (ct0_phys >> 32) as u32,
                rsv:   [0; 4],
            });
        }

        let pb = self.port_base();
        let start = crate::timer::ticks();
        while (read32(pb + PXTFD) & (TFD_BSY | TFD_DRQ)) != 0 {
            if crate::timer::ticks().wrapping_sub(start) > CMD_TIMEOUT {
                return Err(BlockError::Timeout);
            }
            core::hint::spin_loop();
        }
        write32(pb + PXIS, 0xFFFF_FFFF);
        write32(pb + PXCI, 1);
        let start = crate::timer::ticks();
        while (read32(pb + PXCI) & 1) != 0 {
            if crate::timer::ticks().wrapping_sub(start) > CMD_TIMEOUT {
                return Err(BlockError::Timeout);
            }
            if (read32(pb + PXTFD) & TFD_ERR) != 0 { return Err(BlockError::Io); }
            core::hint::spin_loop();
        }
        if (read32(pb + PXTFD) & TFD_ERR) != 0 { return Err(BlockError::Io); }
        Ok(())
    }

    /// READ CAPACITY(10) → numero di blocchi (last_lba + 1). Legge nello scratch.
    fn atapi_read_capacity(&mut self) -> Result<u64, BlockError> {
        let scratch_phys = self.dma.phys.as_u64() + OFF_SCRATCH as u64;
        let cdb = crate::ahci::atapi::read_capacity10_cdb();
        self.issue_atapi(&cdb, scratch_phys, 8)?;
        let scratch_virt = self.dma.virt.as_u64() as usize + OFF_SCRATCH;
        let buf = unsafe { core::slice::from_raw_parts(scratch_virt as *const u8, 8) };
        let (last_lba, _bs) = crate::ahci::atapi::parse_read_capacity10(buf)
            .ok_or(BlockError::Io)?;
        Ok(last_lba as u64 + 1)
    }

    /// Issue IDENTIFY DEVICE into the scratch sector, parse sector count + model.
    fn identify(&mut self) -> Result<(), BlockError> {
        let scratch_phys = self.dma.phys.as_u64() + OFF_SCRATCH as u64;
        self.issue(ATA_IDENTIFY, 0, 1, scratch_phys, 512, false)?;

        // IDENTIFY DATA is 256 words (LE). Parse what we need:
        //   word 83 bit 10  -> LBA48 supported
        //   words 100..104  -> u64 LBA48 sector count
        //   words 27..47    -> model string (40 chars, byte-swapped within each word)
        let scratch_virt = self.dma.virt.as_u64() as usize + OFF_SCRATCH;
        let words: &[u16; 256] = unsafe { &*(scratch_virt as *const [u16; 256]) };

        let lba48 = (words[83] & (1 << 10)) != 0;
        if lba48 {
            self.sectors = u64::from(words[100])
                | (u64::from(words[101]) << 16)
                | (u64::from(words[102]) << 32)
                | (u64::from(words[103]) << 48);
        } else {
            // 28-bit LBA fallback (words 60..62).
            self.sectors = u64::from(words[60]) | (u64::from(words[61]) << 16);
        }

        let mut buf: Vec<u8> = Vec::with_capacity(40);
        for &w in &words[27..47] {
            buf.push((w >> 8) as u8);
            buf.push((w & 0xFF) as u8);
        }
        // Drop trailing spaces / null bytes from the ATA model field.
        while matches!(buf.last(), Some(b' ') | Some(0)) { buf.pop(); }
        self.model = String::from_utf8_lossy(&buf).into_owned();

        Ok(())
    }

    /// Issue FLUSH CACHE EXT — commit the disk's write cache so prior writes are
    /// durable across power-off / VM reset (the install path calls this before
    /// reporting success). No-data command: PRDTL=0, count/lba 0. Cheap; one cmd.
    pub fn flush(&mut self) -> Result<(), BlockError> {
        self.issue(ATA_FLUSH_CACHE_EXT, 0, 0, 0, 0, false)
    }
}

impl BlockDevice for AhciPort {
    fn block_size(&self) -> u32 { if self.is_atapi { ATAPI_BLOCK as u32 } else { 512 } }
    fn block_count(&self) -> u64 { self.sectors }

    fn read_blocks(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        if self.is_atapi {
            if buf.len() % ATAPI_BLOCK != 0 { return Err(BlockError::BadAlignment); }
            let blocks = (buf.len() / ATAPI_BLOCK) as u64;
            if lba.checked_add(blocks).map(|e| e > self.sectors).unwrap_or(true) {
                return Err(BlockError::OutOfRange);
            }
            let mut done = 0u64;
            while done < blocks {
                let chunk = core::cmp::min(blocks - done, 2048) as u16;
                let slice = &mut buf[(done as usize) * ATAPI_BLOCK
                    ..((done + chunk as u64) as usize) * ATAPI_BLOCK];
                let phys = slice.as_ptr() as u64 - crate::memory::mapper::hhdm_offset();
                let cdb = crate::ahci::atapi::read10_cdb((lba + done) as u32, chunk);
                self.issue_atapi(&cdb, phys, chunk as u32 * ATAPI_BLOCK as u32)?;
                done += chunk as u64;
            }
            return Ok(());
        }
        if buf.len() % 512 != 0 { return Err(BlockError::BadAlignment); }
        let sectors = (buf.len() / 512) as u64;
        if lba.checked_add(sectors).map(|e| e > self.sectors).unwrap_or(true) {
            return Err(BlockError::OutOfRange);
        }
        let mut done = 0u64;
        while done < sectors {
            let chunk = core::cmp::min(sectors - done, 8192) as u16;
            let slice = &mut buf[(done as usize) * 512..((done as u64 + chunk as u64) as usize) * 512];
            let phys = slice.as_ptr() as u64 - crate::memory::mapper::hhdm_offset();
            self.issue(ATA_READ_DMA_EXT, lba + done, chunk, phys, chunk as u32 * 512, false)?;
            done += chunk as u64;
        }
        Ok(())
    }

    fn write_blocks(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        if self.is_atapi { return Err(BlockError::Io); } // CD read-only
        if buf.len() % 512 != 0 { return Err(BlockError::BadAlignment); }
        let sectors = (buf.len() / 512) as u64;
        if lba.checked_add(sectors).map(|e| e > self.sectors).unwrap_or(true) {
            return Err(BlockError::OutOfRange);
        }
        let mut done = 0u64;
        while done < sectors {
            let chunk = core::cmp::min(sectors - done, 8192) as u16;
            let slice = &buf[(done as usize) * 512..((done as u64 + chunk as u64) as usize) * 512];
            let phys = slice.as_ptr() as u64 - crate::memory::mapper::hhdm_offset();
            self.issue(ATA_WRITE_DMA_EXT, lba + done, chunk, phys, chunk as u32 * 512, true)?;
            done += chunk as u64;
        }
        Ok(())
    }
}

#[inline]
fn port_addr(abar: VirtAddr, idx: usize) -> usize {
    abar.as_u64() as usize + 0x100 + idx * 0x80
}

#[inline]
fn read32(addr: usize) -> u32 {
    unsafe { read_volatile(addr as *const u32) }
}

#[inline]
fn write32(addr: usize, v: u32) {
    unsafe { write_volatile(addr as *mut u32, v); }
}

/// Wait until `bits` are all zero in the register at `addr`, up to `timeout`
/// ticks. Returns true if cleared, false if timed out.
fn wait_clear(addr: usize, bits: u32, timeout: u64) -> bool {
    let start = crate::timer::ticks();
    while (read32(addr) & bits) != 0 {
        if crate::timer::ticks().wrapping_sub(start) > timeout {
            return false;
        }
        core::hint::spin_loop();
    }
    true
}
