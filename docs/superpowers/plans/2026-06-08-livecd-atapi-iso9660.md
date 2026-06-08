# Live-CD `/bin` da ISO9660 via ATAPI — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Limine carica solo il kernel; `/bin` viene montato e letto on-demand dal CD di boot (filesystem ISO9660, driver ATAPI su AHCI) invece che dai moduli Limine pre-caricati in RAM.

**Architecture:** Nuovo driver ATAPI dentro l'AHCI esistente (PACKET command + CDB SCSI READ(10), settori 2048 B, implementa `BlockDevice`). Nuovo backend VFS ISO9660 read-only (`FsImpl::Iso9660`) montato a `/bin`. La fase `storage` distingue porte SATA (→ `/mnt` FAT32, come oggi) da porte ATAPI (→ `/bin` ISO9660); fallback ai moduli Limine (`modules::mount_all`) se non c'è CD. I consumer (`module_by_name`, shell PATH, launcher `scan_apps`) restano invariati.

**Tech Stack:** Rust `no_std`, AHCI/SATA polling, SCSI MMC (ATAPI), ISO9660, VFS async cooperativo, build Limine/xorriso via Makefile.

**Spec di riferimento:** `docs/superpowers/specs/2026-06-08-livecd-atapi-iso9660-design.md`

**Nota di build/test:** ogni comando build/run gira in WSL:
`wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && <cmd>'`.
Gli unit test del kernel girano su host (`cfg(test)` con `std`, come `blockdev::tests`):
`cargo test -p kernel <filtro>` dentro `kernel/` (vedi come gira già `blockdev`).

> **Nota di scostamento dalla spec:** la spec ipotizzava un file `ahci/atapi.rs`
> autonomo. Per riusare la DMA region e la macchina `issue()` già private in
> `AhciPort`, l'**emissione PACKET vive in `port.rs`** (esteso), mentre `ahci/atapi.rs`
> contiene solo i **builder/parse dei CDB SCSI** (funzioni pure, unit-testabili).
> Stesso risultato funzionale.

---

## File Structure

- **Create** `kernel/src/ahci/atapi.rs` — builder CDB SCSI puri: `read10_cdb`, `read_capacity10_cdb`, `parse_read_capacity10`. Unit-testabili su host.
- **Modify** `kernel/src/ahci/mod.rs` — `pub mod atapi;`; enumerazione multi-HBA + lista porte ATAPI.
- **Modify** `kernel/src/ahci/port.rs` — bringup che accetta signature ATAPI; metodo `issue_atapi`; `is_atapi`; `read_blocks` con ramo 2048 B PACKET; `block_size()` dinamico.
- **Create** `kernel/src/vfs/iso9660.rs` — backend VFS read-only: `Iso9660Fs`, `Iso9660File`, parse PVD + directory record + lookup.
- **Modify** `kernel/src/vfs/fs.rs` — variante `FsImpl::Iso9660`.
- **Modify** `kernel/src/vfs/file.rs` — variante `FileImpl::Iso9660`.
- **Modify** `kernel/src/vfs/mod.rs` — `pub mod iso9660;`.
- **Modify** `kernel/src/boot/phases/storage.rs` — montare `/bin` dal CD ATAPI; ritornare se il CD è stato montato.
- **Modify** `kernel/src/boot/phases/fs.rs` — chiamare `mount_all()` solo come fallback (CD assente).
- **Modify** `Makefile` + `limine.conf` — bin off-boot (sul filesystem ISO, non come moduli).
- **Create** `CHANGELOG/342-...` già esistente; aggiungere entry per il codice (vedi Task 8).

---

## Task 1: CDB SCSI (builder puri, TDD)

**Files:**
- Create: `kernel/src/ahci/atapi.rs`
- Modify: `kernel/src/ahci/mod.rs:13` (aggiungere `pub mod atapi;`)

- [ ] **Step 1: Scrivere il file con i builder + test falliti**

`kernel/src/ahci/atapi.rs`:
```rust
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
        // last_lba = 0x000F_FFFF, block_size = 2048
        let resp = [0x00, 0x0F, 0xFF, 0xFF, 0x00, 0x00, 0x08, 0x00];
        assert_eq!(parse_read_capacity10(&resp), Some((0x000F_FFFF, 2048)));
    }

    #[test] fn capacity_short_buf_none() {
        assert_eq!(parse_read_capacity10(&[0u8; 4]), None);
    }
}
```

In `kernel/src/ahci/mod.rs`, dopo la riga `pub mod port;` (riga 13):
```rust
pub mod atapi;
```

- [ ] **Step 2: Eseguire i test — devono fallire (modulo non ancora compilato/registrato)**

Run (in `kernel/`): `cargo test -p kernel atapi`
Expected: prima FAIL/compile-error finché il modulo non è registrato, poi al passo 1 completo → i 3 test compilano.

- [ ] **Step 3: Eseguire i test — devono passare**

Run: `cargo test -p kernel atapi`
Expected: PASS (`read10_encodes_lba_and_count`, `capacity_roundtrip`, `capacity_short_buf_none`).

- [ ] **Step 4: Commit**

```bash
git add kernel/src/ahci/atapi.rs kernel/src/ahci/mod.rs
git commit -m "feat(ahci): SCSI CDB builders for ATAPI (READ(10)/READ CAPACITY)"
```

---

## Task 2: ATAPI PACKET nel port AHCI + BlockDevice 2048 B

**Files:**
- Modify: `kernel/src/ahci/port.rs` (signature ATAPI in `bringup`, `is_atapi`, `issue_atapi`, `read_blocks`, `block_size`)

Hardware → niente unit test host; verifica via smoke in-boot (Task 5) + e2e (Task 7).

- [ ] **Step 1: Aggiungere costanti ATAPI**

In `kernel/src/ahci/port.rs`, accanto a `const SIG_SATA` (riga 46):
```rust
// ATAPI device signature reported in PxSIG (CD-ROM, packet device).
const SIG_ATAPI: u32 = 0xEB14_0101;

// ATA PACKET command + DMA feature bit.
const ATA_PACKET: u8 = 0xA0;
const PACKET_FEATURE_DMA: u8 = 1 << 0;

// Command-header flag bit A (ATAPI) — bit 5 of `flags`.
const CH_FLAG_ATAPI: u16 = 1 << 5;

// CD-ROM logical block size.
const ATAPI_BLOCK: usize = 2048;
```

- [ ] **Step 2: Aggiungere il campo `is_atapi` allo struct**

Modificare `pub struct AhciPort` (riga 121) aggiungendo il campo:
```rust
pub struct AhciPort {
    abar:      VirtAddr,
    port_idx:  usize,
    dma:       DmaRegion,
    pub sectors: u64,
    pub model:   String,
    pub is_atapi: bool,
}
```
E nel costruttore dentro `bringup` (riga 214) inizializzarlo a `false` per default:
```rust
let mut p = Self { abar, port_idx, dma, sectors: 0, model: String::new(), is_atapi: false };
```

- [ ] **Step 3: Accettare la signature ATAPI in `bringup`**

Sostituire il blocco che oggi rifiuta i non-SATA (righe 204-208):
```rust
        let sig = read32(port_base + PXSIG);
        if sig != SIG_SATA {
            crate::bwarn!("ahci", "port {} non-SATA sig=0x{:08x}", port_idx, sig);
            return None;
        }
```
con:
```rust
        let sig = read32(port_base + PXSIG);
        let is_atapi = match sig {
            SIG_SATA  => false,
            SIG_ATAPI => true,
            other => {
                crate::bwarn!("ahci", "port {} unknown sig=0x{:08x}", port_idx, other);
                return None;
            }
        };
```
Aggiornare il costruttore (passo 2) a usare `is_atapi`:
```rust
        let mut p = Self { abar, port_idx, dma, sectors: 0, model: String::new(), is_atapi };
```
E saltare IDENTIFY DEVICE per ATAPI (IDENTIFY DEVICE è solo ATA; per il CD usiamo READ CAPACITY). Sostituire il blocco IDENTIFY (righe 216-225) con:
```rust
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
```
(La `cache_disk_info` alla riga 231 resta invariata.)

- [ ] **Step 4: Aggiungere `issue_atapi` + `atapi_read_capacity`**

Dopo il metodo `issue` (chiude a riga 358), aggiungere nell'`impl AhciPort`:
```rust
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
            // Byte-count limit (lba1/lba2) — usato in PIO; in DMA il device usa la PRDT.
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
            // CDB → campo ACMD (12 byte significativi, resto zero).
            let acmd_ptr = core::ptr::addr_of_mut!((*ct).acmd) as *mut u8;
            core::ptr::write_bytes(acmd_ptr, 0, 16);
            core::ptr::copy_nonoverlapping(cdb.as_ptr(), acmd_ptr, 12);
            // PRDT[0] → buffer dati.
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
        // CFL + flag ATAPI; W=0 (read).
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
```

- [ ] **Step 5: `block_size` dinamico + ramo ATAPI in `read_blocks`**

Sostituire l'`impl BlockDevice for AhciPort` (righe 403-441) — `block_size` e `read_blocks` cambiano; `write_blocks` su ATAPI ritorna errore:
```rust
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
            // Una PRDT entry copre 4 MiB = 2048 blocchi da 2048 B. Spezza oltre.
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
```

- [ ] **Step 6: Compilare il kernel (target reale)**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && cargo build -p kernel --target x86_64-unknown-none 2>&1 | tail -20'`
Expected: compila senza errori (warning ok).

- [ ] **Step 7: Commit**

```bash
git add kernel/src/ahci/port.rs
git commit -m "feat(ahci): ATAPI PACKET read path + 2048B BlockDevice on AHCI port"
```

---

## Task 3: Enumerazione multi-HBA + porte ATAPI

**Files:**
- Modify: `kernel/src/ahci/mod.rs` (scan di tutti i controller AHCI + lista porte ATAPI)

> **Perché:** in q35 ci sono DUE controller AHCI (ICH9 builtin col CD + `-device ahci` con l'hd). `Hba::find_and_init` (oggi) prende il primo match; il CD potrebbe stare sull'altro. Servono: (a) trovare anche il secondo ABAR; (b) un helper che, dato l'ABAR, elenchi le porte ATAPI.

- [ ] **Step 1: Aggiungere un helper che porta su le porte ATAPI del boot HBA**

In `kernel/src/ahci/mod.rs`, dopo `sata_ports()` (riga 102), aggiungere:
```rust
/// Porta su (bringup) la prima porta ATAPI (CD-ROM) trovata sul boot HBA.
/// `None` se nessuna porta presenta la signature ATAPI. Usato da
/// `boot::phases::storage` per montare `/bin` dal CD live.
pub fn acquire_atapi_port() -> Option<AhciPort> {
    let (abar, pi) = (*BOOT_HBA.lock())?;
    for idx in 0..32 {
        if pi & (1 << idx) == 0 { continue; }
        if let Some(port) = AhciPort::bringup(abar, idx) {
            if port.is_atapi { return Some(port); }
        }
    }
    None
}
```

> **Nota multi-HBA:** se in QEMU il CD finisce su un AHCI diverso da quello che
> `find_and_init` sceglie, il bringup sopra non lo vede. Verificare empiricamente
> al Task 7: nei log `make run-test`, se compare `port N atapi sectors=...` →
> singolo HBA basta. Se NON compare ma il CD esiste, estendere `hba.rs`
> (`find_and_init`) per enumerare TUTTI i match `pci::find_class(0x01,0x06,0x01)`
> e provarli in ordine. Tenere questo come step condizionato dall'osservazione,
> non implementare a vuoto (YAGNI finché il log non lo richiede).

- [ ] **Step 2: Compilare**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && cargo build -p kernel --target x86_64-unknown-none 2>&1 | tail -20'`
Expected: compila senza errori.

- [ ] **Step 3: Commit**

```bash
git add kernel/src/ahci/mod.rs
git commit -m "feat(ahci): acquire_atapi_port — bring up first ATAPI CD-ROM on boot HBA"
```

---

## Task 4: Backend VFS ISO9660 read-only (TDD)

**Files:**
- Create: `kernel/src/vfs/iso9660.rs`
- Modify: `kernel/src/vfs/mod.rs:11` (`pub mod iso9660;`)
- Modify: `kernel/src/vfs/fs.rs` (variante `FsImpl::Iso9660` + instradamento)
- Modify: `kernel/src/vfs/file.rs` (variante `FileImpl::Iso9660`)

- [ ] **Step 1: Scrivere `iso9660.rs` con parsing + test falliti**

`kernel/src/vfs/iso9660.rs`:
```rust
//! Filesystem ISO9660 read-only (live-CD).
//!
//! Supporta il sufficiente per leggere `/bin/*.wasm`/`.cwasm` dal CD di boot:
//! Primary Volume Descriptor (settore 16), traversata directory via directory
//! record, file come extent contiguo. Niente Joliet/Rock Ridge nel primo taglio
//! (nomi 8.3 + `;1`). Backed da qualunque `BlockDevice` a 2048 B.
//!
//! Tutte le scritture → `VfsError::Unsupported` (CD read-only).

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::blockdev::{BlockDevice, BlockError};
use crate::vfs::error::VfsError;
use crate::vfs::file::{File, FileImpl, OpenFlags, Whence};
use crate::vfs::fs::{FileSystem, VfsDirent, VfsKind, VfsStat};

const ISO_SECTOR: usize = 2048;
const PVD_LBA: u64 = 16;

/// Un directory record ISO9660 parsato (solo i campi che usiamo).
#[derive(Debug, Clone)]
struct IsoEntry {
    name: String,   // normalizzato lowercase, senza ";1"
    is_dir: bool,
    extent_lba: u32,
    size: u32,
}

struct IsoInner {
    dev: Box<dyn BlockDevice + Send>,
    root_lba: u32,
    root_size: u32,
    /// Componenti del sottopercorso ISO da anteporre (es. ["bin"]) — la mount
    /// mappa il prefisso VFS `/bin` su `/bin` dell'ISO.
    base: Vec<String>,
}

impl IsoInner {
    fn read_sector(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), VfsError> {
        self.dev.read_blocks(lba, buf).map_err(map_block_err)
    }

    /// Legge l'extent (`size` byte a partire da `lba`) in un Vec.
    fn read_extent(&mut self, lba: u32, size: u32) -> Result<Vec<u8>, VfsError> {
        let sectors = ((size as usize) + ISO_SECTOR - 1) / ISO_SECTOR;
        let mut out = alloc::vec![0u8; sectors * ISO_SECTOR];
        for i in 0..sectors {
            let s = &mut out[i * ISO_SECTOR..(i + 1) * ISO_SECTOR];
            self.read_sector(lba as u64 + i as u64, s)?;
        }
        out.truncate(size as usize);
        Ok(out)
    }
}

/// Parsa un blocco di directory record (l'extent di una dir) in voci.
/// I record non attraversano i confini di settore: un `len==0` salta al
/// prossimo confine di 2048 B.
fn parse_dir(extent: &[u8]) -> Vec<IsoEntry> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off < extent.len() {
        let len = extent[off] as usize;
        if len == 0 {
            // Salta al prossimo confine di settore.
            let next = (off / ISO_SECTOR + 1) * ISO_SECTOR;
            if next <= off { break; }
            off = next;
            continue;
        }
        if off + len > extent.len() { break; }
        let rec = &extent[off..off + len];
        // extent location: both-endian a offset 2, LE in [2..6].
        let extent_lba = u32::from_le_bytes([rec[2], rec[3], rec[4], rec[5]]);
        // data length: both-endian a offset 10, LE in [10..14].
        let size = u32::from_le_bytes([rec[10], rec[11], rec[12], rec[13]]);
        let flags = rec[25];
        let is_dir = flags & 0x02 != 0;
        let name_len = rec[32] as usize;
        let raw = &rec[33..33 + name_len.min(rec.len() - 33)];
        // Voci "." (0x00) e ".." (0x01): salta.
        if name_len == 1 && (raw[0] == 0 || raw[0] == 1) {
            off += len;
            continue;
        }
        let mut name = String::from_utf8_lossy(raw).into_owned();
        // Rimuovi ";1" version suffix.
        if let Some(p) = name.find(';') { name.truncate(p); }
        let name = name.to_ascii_lowercase();
        out.push(IsoEntry { name, is_dir, extent_lba, size });
        off += len;
    }
    out
}

fn map_block_err(_e: BlockError) -> VfsError { VfsError::IoError }

pub struct Iso9660Fs {
    inner: Arc<Mutex<IsoInner>>,
}

impl Iso9660Fs {
    /// Monta un ISO9660 da `dev`, esponendo il sottoalbero `base` (es. "/bin").
    pub fn from_blockdev(
        mut dev: Box<dyn BlockDevice + Send>,
        base: &str,
    ) -> Result<Self, VfsError> {
        let mut pvd = [0u8; ISO_SECTOR];
        dev.read_blocks(PVD_LBA, &mut pvd).map_err(map_block_err)?;
        // type==1 (primary) + magic "CD001".
        if pvd[0] != 1 || &pvd[1..6] != b"CD001" {
            return Err(VfsError::IoError);
        }
        // Root directory record a offset 156 (lunghezza 34).
        let root = &pvd[156..156 + 34];
        let root_lba = u32::from_le_bytes([root[2], root[3], root[4], root[5]]);
        let root_size = u32::from_le_bytes([root[10], root[11], root[12], root[13]]);
        let base: Vec<String> = base.split('/').filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_lowercase()).collect();
        crate::binfo!("iso9660", "PVD ok root_lba={} root_size={} base={:?}",
            root_lba, root_size, base);
        Ok(Self { inner: Arc::new(Mutex::new(IsoInner {
            dev, root_lba, root_size, base,
        })) })
    }

    /// Risolve i componenti (sotto `base`) a una voce. `parts` è relativo al
    /// prefisso VFS; vi anteponiamo `base` per ottenere il path ISO assoluto.
    fn lookup(&self, parts: &[&str]) -> Result<IsoEntry, VfsError> {
        let mut inner = self.inner.lock();
        let mut full: Vec<String> = inner.base.clone();
        full.extend(parts.iter().map(|s| s.to_ascii_lowercase()));

        let mut cur_lba = inner.root_lba;
        let mut cur_size = inner.root_size;
        let mut last: Option<IsoEntry> = None;
        for (i, comp) in full.iter().enumerate() {
            let extent = inner.read_extent(cur_lba, cur_size)?;
            let entries = parse_dir(&extent);
            let found = entries.into_iter().find(|e| &e.name == comp)
                .ok_or(VfsError::NotFound)?;
            let is_last = i == full.len() - 1;
            if !is_last {
                if !found.is_dir { return Err(VfsError::NotDirectory); }
                cur_lba = found.extent_lba;
                cur_size = found.size;
            }
            last = Some(found);
        }
        // full vuoto (parts vuoto, base vuoto) → root dir.
        last.ok_or(VfsError::NotFound)
    }
}

impl FileSystem for Iso9660Fs {
    async fn open(&self, path: &[&str], _flags: OpenFlags) -> Result<FileImpl, VfsError> {
        let e = self.lookup(path)?;
        if e.is_dir { return Err(VfsError::IsDirectory); }
        Ok(FileImpl::Iso9660(Iso9660File {
            fs: Arc::clone(&self.inner),
            extent_lba: e.extent_lba,
            size: e.size,
            pos: 0,
        }))
    }

    async fn create(&self, _path: &[&str]) -> Result<(), VfsError> { Err(VfsError::Unsupported) }
    async fn unlink(&self, _path: &[&str]) -> Result<(), VfsError> { Err(VfsError::Unsupported) }
    async fn mkdir(&self, _path: &[&str]) -> Result<(), VfsError> { Err(VfsError::Unsupported) }
    async fn rmdir(&self, _path: &[&str]) -> Result<(), VfsError> { Err(VfsError::Unsupported) }
    async fn rename(&self, _src: &[&str], _dst: &[&str]) -> Result<(), VfsError> { Err(VfsError::Unsupported) }

    async fn readdir(&self, path: &[&str]) -> Result<Vec<VfsDirent>, VfsError> {
        let (lba, size) = if path.is_empty() {
            let inner = self.inner.lock();
            // readdir della root del prefisso = la dir `base` sull'ISO.
            if inner.base.is_empty() {
                (inner.root_lba, inner.root_size)
            } else {
                drop(inner);
                let e = self.lookup(&[])?;
                (e.extent_lba, e.size)
            }
        } else {
            let e = self.lookup(path)?;
            if !e.is_dir { return Err(VfsError::NotDirectory); }
            (e.extent_lba, e.size)
        };
        let mut inner = self.inner.lock();
        let extent = inner.read_extent(lba, size)?;
        Ok(parse_dir(&extent).into_iter().map(|e| VfsDirent {
            name: e.name,
            kind: if e.is_dir { VfsKind::Dir } else { VfsKind::Reg },
        }).collect())
    }

    async fn stat(&self, path: &[&str]) -> Result<VfsStat, VfsError> {
        if path.is_empty() {
            return Ok(VfsStat { kind: VfsKind::Dir, size: 0 });
        }
        let e = self.lookup(path)?;
        Ok(VfsStat {
            kind: if e.is_dir { VfsKind::Dir } else { VfsKind::Reg },
            size: e.size as u64,
        })
    }
}

pub struct Iso9660File {
    fs: Arc<Mutex<IsoInner>>,
    extent_lba: u32,
    size: u32,
    pos: u64,
}

impl File for Iso9660File {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, VfsError> {
        if self.pos >= self.size as u64 || buf.is_empty() { return Ok(0); }
        let mut inner = self.fs.lock();
        let sector_in_file = self.pos / ISO_SECTOR as u64;
        let within = (self.pos % ISO_SECTOR as u64) as usize;
        let mut sbuf = [0u8; ISO_SECTOR];
        inner.read_sector(self.extent_lba as u64 + sector_in_file, &mut sbuf)?;
        let avail_in_sector = ISO_SECTOR - within;
        let avail_in_file = (self.size as u64 - self.pos) as usize;
        let n = buf.len().min(avail_in_sector).min(avail_in_file);
        buf[..n].copy_from_slice(&sbuf[within..within + n]);
        self.pos += n as u64;
        Ok(n)
    }

    async fn write(&mut self, _buf: &[u8]) -> Result<usize, VfsError> { Err(VfsError::Unsupported) }

    async fn seek(&mut self, off: i64, whence: Whence) -> Result<u64, VfsError> {
        let base = match whence {
            Whence::Set => 0i64,
            Whence::Cur => self.pos as i64,
            Whence::End => self.size as i64,
        };
        let np = base.checked_add(off).ok_or(VfsError::InvalidPath)?;
        if np < 0 { return Err(VfsError::InvalidPath); }
        self.pos = np as u64;
        Ok(self.pos)
    }

    async fn stat(&self) -> Result<VfsStat, VfsError> {
        Ok(VfsStat { kind: VfsKind::Reg, size: self.size as u64 })
    }
}

/// Monta un ISO9660 da un block device esponendo `iso_base` al prefisso `prefix`.
pub fn mount_from_blockdev(
    dev: Box<dyn BlockDevice + Send>,
    prefix: &str,
    iso_base: &str,
) -> Result<(), VfsError> {
    let fs = Iso9660Fs::from_blockdev(dev, iso_base)?;
    crate::vfs::mount(prefix, crate::vfs::fs::FsImpl::Iso9660(fs))
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec; use std::vec::Vec as StdVec;

    // Block device a 2048 B su un Vec, per costruire un ISO sintetico.
    struct MemCd(StdVec<u8>);
    impl BlockDevice for MemCd {
        fn block_size(&self) -> u32 { 2048 }
        fn block_count(&self) -> u64 { (self.0.len() / 2048) as u64 }
        fn read_blocks(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
            let o = (lba as usize) * 2048;
            buf.copy_from_slice(&self.0[o..o + buf.len()]); Ok(())
        }
        fn write_blocks(&mut self, _lba: u64, _buf: &[u8]) -> Result<(), BlockError> {
            Err(BlockError::Io)
        }
    }

    // Costruisce un directory record minimale.
    fn dir_record(name: &[u8], lba: u32, size: u32, is_dir: bool) -> StdVec<u8> {
        let len = 33 + name.len();
        let len = if len % 2 == 1 { len + 1 } else { len }; // padding pari
        let mut r = vec![0u8; len];
        r[0] = len as u8;
        r[2..6].copy_from_slice(&lba.to_le_bytes());
        r[10..14].copy_from_slice(&size.to_le_bytes());
        r[25] = if is_dir { 0x02 } else { 0x00 };
        r[32] = name.len() as u8;
        r[33..33 + name.len()].copy_from_slice(name);
        r
    }

    // ISO: settore16=PVD; settore20=root dir (contiene "BIN"); settore21=bin dir
    // (contiene "LS.WASM;1"); settore22=contenuto file.
    fn build_iso() -> StdVec<u8> {
        let mut img = vec![0u8; 2048 * 24];
        // PVD
        img[16 * 2048] = 1;
        img[16 * 2048 + 1..16 * 2048 + 6].copy_from_slice(b"CD001");
        let root = dir_record(&[0u8], 20, 2048, true); // root record con name "\0"
        img[16 * 2048 + 156..16 * 2048 + 156 + root.len()].copy_from_slice(&root);
        // root dir @ lba20: voce "BIN" -> lba21
        let mut off = 20 * 2048;
        let bin = dir_record(b"BIN", 21, 2048, true);
        img[off..off + bin.len()].copy_from_slice(&bin); off += bin.len();
        let _ = off;
        // bin dir @ lba21: voce "LS.WASM;1" -> lba22, size 5
        let ls = dir_record(b"LS.WASM;1", 22, 5, false);
        img[21 * 2048..21 * 2048 + ls.len()].copy_from_slice(&ls);
        // file content @ lba22
        img[22 * 2048..22 * 2048 + 5].copy_from_slice(b"hello");
        img
    }

    #[test] fn parse_pvd_and_lookup_file() {
        let fs = Iso9660Fs::from_blockdev(Box::new(MemCd(build_iso())), "/bin").unwrap();
        let e = fs.lookup(&["ls.wasm"]).unwrap();
        assert!(!e.is_dir);
        assert_eq!(e.extent_lba, 22);
        assert_eq!(e.size, 5);
    }

    #[test] fn lookup_missing_is_notfound() {
        let fs = Iso9660Fs::from_blockdev(Box::new(MemCd(build_iso())), "/bin").unwrap();
        assert!(matches!(fs.lookup(&["nope.wasm"]), Err(VfsError::NotFound)));
    }

    #[test] fn bad_magic_rejected() {
        let img = vec![0u8; 2048 * 20];
        assert!(Iso9660Fs::from_blockdev(Box::new(MemCd(img)), "/bin").is_err());
    }
}
```

- [ ] **Step 2: Registrare il modulo + le varianti enum**

In `kernel/src/vfs/mod.rs`, dopo `pub mod fat32;` (riga 11):
```rust
pub mod iso9660;
```

In `kernel/src/vfs/fs.rs`: aggiungere l'import (dopo riga 34) e la variante (dopo riga 38), poi instradare TUTTI i metodi. Import:
```rust
use crate::vfs::iso9660::Iso9660Fs;
```
Variante enum:
```rust
pub enum FsImpl {
    Tmpfs(Tmpfs),
    Fat32(Fat32Fs),
    Iso9660(Iso9660Fs),
}
```
In ognuno dei match (`open`, `create`, `unlink`, `readdir`, `stat`) aggiungere il braccio. Esempio per `open` (riga 43-47), aggiungere prima della chiusura:
```rust
            FsImpl::Iso9660(i) => i.open(path, flags).await,
```
Per `create`:
```rust
            FsImpl::Iso9660(i) => i.create(path).await,
```
Per `unlink`:
```rust
            FsImpl::Iso9660(i) => i.unlink(path).await,
```
Per `readdir`:
```rust
            FsImpl::Iso9660(i) => i.readdir(path).await,
```
Per `stat`:
```rust
            FsImpl::Iso9660(i) => i.stat(path).await,
```
Per `mkdir`/`rmdir`/`rename` (usano la sintassi `<Tipo as FileSystem>::`):
```rust
            FsImpl::Iso9660(i) => <Iso9660Fs as FileSystem>::mkdir(i, path).await,
```
```rust
            FsImpl::Iso9660(i) => <Iso9660Fs as FileSystem>::rmdir(i, path).await,
```
```rust
            FsImpl::Iso9660(i) => <Iso9660Fs as FileSystem>::rename(i, src, dst).await,
```

In `kernel/src/vfs/file.rs`: aggiungere l'import (dopo riga 35) e la variante (dopo riga 44), poi instradare `read`/`write`/`stat`/`seek`. Import:
```rust
use crate::vfs::iso9660::Iso9660File;
```
Variante:
```rust
    Iso9660(Iso9660File),
```
In ognuno dei 4 match (`read`, `write`, `stat`, `seek`) aggiungere:
```rust
            FileImpl::Iso9660(f) => f.read(buf).await,
```
```rust
            FileImpl::Iso9660(f) => f.write(buf).await,
```
```rust
            FileImpl::Iso9660(f) => f.stat().await,
```
```rust
            FileImpl::Iso9660(f) => f.seek(off, whence).await,
```

- [ ] **Step 3: Eseguire i test ISO9660 (host)**

Run (in `kernel/`): `cargo test -p kernel iso9660`
Expected: PASS (`parse_pvd_and_lookup_file`, `lookup_missing_is_notfound`, `bad_magic_rejected`).

> Se `cargo test -p kernel` non gira per via di dipendenze `no_std`, usare lo
> stesso meccanismo con cui girano oggi `blockdev::tests`/`fat32` (i test usano
> `extern crate std` sotto `cfg(test)`). Replicare quella configurazione.

- [ ] **Step 4: Compilare il kernel (target reale)**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && cargo build -p kernel --target x86_64-unknown-none 2>&1 | tail -20'`
Expected: compila senza errori.

- [ ] **Step 5: Commit**

```bash
git add kernel/src/vfs/iso9660.rs kernel/src/vfs/mod.rs kernel/src/vfs/fs.rs kernel/src/vfs/file.rs
git commit -m "feat(vfs): ISO9660 read-only backend (PVD/dir parse) + FsImpl wiring"
```

---

## Task 5: Mount `/bin` dal CD nella fase storage + fallback moduli

**Files:**
- Modify: `kernel/src/boot/phases/storage.rs`
- Modify: `kernel/src/boot/phases/fs.rs` (chiamare `mount_all()` solo se il CD non ha montato `/bin`)

> **Ordine fasi:** `fs` (riga `crate::modules::mount_all()`) gira PRIMA di `storage`.
> Per montare `/bin` dal CD prima del compositor e poter saltare `mount_all`,
> spostiamo il mount CD a `storage` e rendiamo `mount_all` condizionato.
> Verificare l'ordine in `boot/phases/mod.rs`: `storage` deve precedere `userland`
> (che avvia il compositor). Confermato dalla descrizione di `storage.rs`
> ("before userland").

- [ ] **Step 1: In `fs.rs`, rimuovere la chiamata incondizionata a `mount_all`**

In `kernel/src/boot/phases/fs.rs`, alla riga 47, sostituire:
```rust
    crate::modules::mount_all();
    crate::binfo!("fs", "modules mounted");
```
con un commento che rimanda allo storage (il mount avviene là, con fallback):
```rust
    // I bin NON vengono più montati qui dai moduli Limine: la fase `storage`
    // monta `/bin` dal CD live (ISO9660/ATAPI) e, SOLO se non c'è CD, fa il
    // fallback a `modules::mount_all()`. Lasciamo intatti i `/dev` e la root.
    crate::binfo!("fs", "bin mount deferred to storage phase");
```

- [ ] **Step 2: In `storage.rs`, montare `/bin` dal CD e gestire il fallback**

In `kernel/src/boot/phases/storage.rs`, dentro `init()`, DOPO `let hba = ...` (riga 12-15) e PRIMA del loop sulle porte SATA, inserire il tentativo di mount CD:
```rust
    // Live-CD: prova a montare `/bin` dal CD-ROM (ATAPI/ISO9660). Se riesce, i
    // bin arrivano on-demand dal CD e NON serve copiare i moduli Limine in RAM.
    let mut bin_mounted = false;
    if let Some(cd) = crate::ahci::acquire_atapi_port() {
        match crate::vfs::iso9660::mount_from_blockdev(
            alloc::boxed::Box::new(cd), "/bin", "/bin",
        ) {
            Ok(()) => {
                crate::binfo!("storage", "live-cd: /bin mounted from ISO9660 (ATAPI)");
                bin_mounted = true;
            }
            Err(e) => crate::bwarn!("storage", "ISO9660 mount /bin failed: {}", e),
        }
    }

    // Fallback: nessun CD utilizzabile → i bin vengono dai moduli Limine (boot
    // installato su SSD o ISO legacy). Comportamento storico invariato.
    if !bin_mounted {
        let n = crate::modules::mount_all();
        crate::binfo!("storage", "no live-cd: mounted {} boot modules into tmpfs /bin", n);
    }
```

> **Attenzione import:** `storage.rs` usa già `alloc::vec!`; verificare che
> `extern crate alloc;` / `use alloc::boxed::Box;` sia disponibile nel modulo
> (è in `crate` root). Se serve, scrivere il path completo `alloc::boxed::Box::new`
> come sopra (già fatto).

- [ ] **Step 3: Build ISO + run-test (verifica integrata)**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | tail -15 && make run-test 2>&1 | tail -40'`
Expected (in questa fase i bin sono ANCORA moduli — Task 6 li toglie):
- compila e builda l'ISO;
- nel log compare `port N atapi sectors=...` (il CD è visto) **oppure**, se l'ATAPI non è su questo HBA, NON compare → seguire la nota multi-HBA del Task 3 Step 1;
- se `/bin mounted from ISO9660` compare → il path CD funziona; altrimenti fallback ai moduli (test storico passa comunque).

- [ ] **Step 4: Commit**

```bash
git add kernel/src/boot/phases/storage.rs kernel/src/boot/phases/fs.rs
git commit -m "feat(boot): mount /bin from live-CD (ISO9660/ATAPI), fallback to Limine modules"
```

---

## Task 6: Build — bin off-boot (Makefile + limine.conf)

**Files:**
- Modify: `limine.conf` (rimuovere i `module_path` dei bin; tenere kernel + `/payload/*`)
- Modify: `Makefile` (mettere i bin nel filesystem ISO sotto `iso_root/bin/`, non come moduli)

> **Solo a Task 5 verificato** che il mount CD funziona. Questo task spegne i
> moduli: dopo, i bin esistono SOLO sul filesystem ISO → se il mount CD non
> funzionasse, il sistema resterebbe senza `/bin`. Procedere solo dopo che il log
> del Task 5 mostra `/bin mounted from ISO9660`.

- [ ] **Step 1: Rimuovere i bin da `limine.conf`**

In `limine.conf`, eliminare TUTTE le coppie `module_path: boot():/bin/*` +
`module_cmdline: /bin/*`, e anche `/init.wasm`, `/root/server.wasm`,
`/root/client.wasm`, `/etc/init.sh`. **TENERE**: la riga `module_path: boot():/boot/kernel`
iniziale (se presente come kernel) e i quattro `/payload/*` finali
(kernel/BOOTX64.EFI/limine.conf/limine-ssd.conf) usati dall'installer.

- [ ] **Step 2: Adeguare il Makefile**

Nel target che assembla l'ISO (`make iso` / la regola che popola `$(ISO_ROOT)`),
assicurare che i bin vengano COPIATI in `$(ISO_ROOT)/bin/` (filesystem ISO9660 via
xorriso) ma NON aggiunti come moduli a `limine.conf`. Concretamente:
- mantenere le `cp .../$*.wasm $(ISO_ROOT)/bin/` e le `cp build/<id>.cwasm $(ISO_ROOT)/bin/`;
- mettere `init.sh`/`init.wasm` sotto i loro path in `$(ISO_ROOT)`;
- nessun cambiamento alla riga `xorriso` (i file in `iso_root` finiscono già nel
  filesystem ISO9660).

> Individuare le righe esatte con: `grep -n 'ISO_ROOT' Makefile`. Spostare ogni
> `cp` che oggi serve a popolare i moduli verso `$(ISO_ROOT)/bin/`; rimuovere ogni
> generazione automatica di righe `module_path /bin/*` se presente.

- [ ] **Step 3: Build + run-test — i bin ora vengono SOLO dal CD**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | tail -15 && make run-test 2>&1 | tail -50'`
Expected:
- `/bin mounted from ISO9660 (ATAPI)` nel log;
- NESSUNA riga `mod mounted /bin/...` (i moduli bin non esistono più);
- la stringa di successo di `make run-test` (vedi `HELLO`) presente;
- desktop reale (shell dal CD), non fallback egui-demo.

- [ ] **Step 4: Commit**

```bash
git add limine.conf Makefile
git commit -m "build(livecd): ship bins on ISO9660 filesystem, drop them as Limine modules"
```

---

## Task 7: Verifica end-to-end (QEMU live-CD)

**Files:**
- Verifica/Modify: target di test in `Makefile` (estendere `make run-test` con asserzioni live-CD, oppure aggiungere `make run-test-livecd`)

- [ ] **Step 1: Aggiungere asserzioni live-CD al test headless**

Nel blocco di asserzioni di `make run-test` (vedi `Makefile` righe ~276-278 che
grep-pano `build/serial.log`), aggiungere:
```bash
	grep -qF "/bin mounted from ISO9660" build/serial.log || { echo TEST_FAIL_LIVECD_MOUNT; exit 1; }
	grep -qE "port [0-9]+ atapi sectors=" build/serial.log || { echo TEST_FAIL_ATAPI; exit 1; }
```
(Se preferito, isolare in un nuovo target `run-test-livecd` per non bloccare il
test storico durante lo sviluppo.)

- [ ] **Step 2: Eseguire il test e verificare l'output**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -60'`
Expected: niente `TEST_FAIL_*`, stringa di successo presente.

- [ ] **Step 3: Run interattivo manuale (facoltativo, conferma desktop + exec)**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run'`
Verificare a mano: desktop egui reale; aprire il terminale; eseguire `ls` (bin letto
on-demand dal CD); aprire un'app (`about`/`files`) → spawn dal CD.

- [ ] **Step 4: Commit**

```bash
git add Makefile
git commit -m "test(livecd): assert /bin mounts from ISO9660 + ATAPI port in run-test"
```

---

## Task 8: CHANGELOG dell'implementazione

**Files:**
- Create: `CHANGELOG/343-26-06-08-livecd-atapi-iso9660-impl.md`

> Il numero 342 è già usato dalla spec. Verificare il massimo con
> `ls CHANGELOG/ | sort -n | tail -1` e usare il successivo (qui assunto 343).

- [ ] **Step 1: Scrivere l'entry**

`CHANGELOG/343-26-06-08-livecd-atapi-iso9660-impl.md`:
```markdown
# 343 — Live-CD: /bin da ISO9660 via ATAPI (implementazione)

**Data:** 2026-06-08

## Cosa
Driver ATAPI (PACKET/READ(10) su AHCI, settori 2048B) + backend VFS ISO9660
read-only + mount `/bin` dal CD live nella fase storage (fallback ai moduli
Limine se non c'è CD). Bin tolti da limine.conf e spediti sul filesystem ISO9660.

## Perché
Boot più elegante e meno RAM: Limine carica solo il kernel; i bin (~80 CLI + 5 app
.cwasm) si leggono on-demand dal CD invece di essere pre-caricati e ricopiati in
tmpfs.

## File toccati
- kernel/src/ahci/atapi.rs (nuovo)
- kernel/src/ahci/{mod.rs,port.rs}
- kernel/src/vfs/iso9660.rs (nuovo)
- kernel/src/vfs/{mod.rs,fs.rs,file.rs}
- kernel/src/boot/phases/{storage.rs,fs.rs}
- Makefile, limine.conf
```

- [ ] **Step 2: Commit**

```bash
git add CHANGELOG/343-26-06-08-livecd-atapi-iso9660-impl.md
git commit -m "docs(changelog): 343 live-cd ATAPI/ISO9660 implementation"
```

---

## Self-Review (eseguito)

**1. Spec coverage:**
- §4.1 driver ATAPI → Task 1 (CDB) + Task 2 (PACKET/BlockDevice). ✓
- §4.2 ISO9660 + `FsImpl::Iso9660` → Task 4. ✓
- §4.3 fase mount + fallback + ordine fasi → Task 5; multi-HBA → Task 3. ✓
- §4.4 build off-boot → Task 6. ✓
- §9 test (smoke ATAPI, unit ISO9660, e2e) → Task 1/4 (unit), Task 5/7 (smoke+e2e). ✓
- §8 rischi: ordine mount (Task 5 nota), due HBA (Task 3 nota), nomi 8.3 (vedi sotto). ✓

**2. Placeholder scan:** nessun TBD/TODO; ogni step ha codice o comando concreto.
La nota multi-HBA (Task 3) è condizionata all'osservazione del log per YAGNI, con
azione esatta indicata — non è un placeholder.

**3. Type consistency:** `Iso9660Fs`/`Iso9660File`/`FsImpl::Iso9660`/`FileImpl::Iso9660`,
`acquire_atapi_port`, `mount_from_blockdev(dev, prefix, iso_base)`, `is_atapi`,
`issue_atapi`, `read10_cdb`/`read_capacity10_cdb`/`parse_read_capacity10`: coerenti
tra i task. `VfsError::Unsupported` usato per le scritture RO (esiste nell'enum).

**Questione aperta (nomi ISO9660 8.3):** se xorriso scrive nomi che non rientrano
in 8.3 (es. `compositor.cwasm` → 12 char base) il primo parser potrebbe non
trovarli. Mitigazione nel Task 6 Step 3: il log mostrerà `shell.cwasm` not found →
in tal caso aggiungere parsing **Joliet** (Supplementary Volume Descriptor a UCS-2,
settore 17) come task di follow-up. Tenuto fuori dal primo taglio (YAGNI finché il
test non lo richiede), ma segnalato.
```
