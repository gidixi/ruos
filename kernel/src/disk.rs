//! Disk authoring (M2a): GPT + FAT format + dir tree on a raw block device.
//!
//! Ties together the M2a write-side building blocks — `gpt::write_layout`
//! (Task 2), `vfs::fat32::format` (Task 3) and `vfs::fat32::create_dirs`
//! (Task 4/5a) — into a single destructive "lay down a fresh ruos disk"
//! operation. It works from only a borrow of the raw disk: `PartBorrow` carves
//! out each partition region so `format`/`create_dirs` see a clean LBA-0-based
//! view, with the same range checks as the owning `PartitionDevice`.
//!
//! Deliberately NOT routed through `Fat32Fs` / the async `mkdir` / `vfs::mount`:
//! authoring is transient and synchronous and must not touch the global `/mnt`
//! mount table.

use crate::blockdev::{BlockDevice, PartBorrow};
use crate::gpt::Extent;

/// Why `author` could not lay down the disk.
#[derive(Debug)]
pub enum DiskError {
    /// The device is too small (or sized oddly) for the requested layout.
    TooSmall,
    /// A format / dir-tree / block I/O step failed.
    Io,
}

/// The partition placement `author` produced.
pub struct Layout {
    pub esp: Extent,
    pub data: Extent,
}

/// Author a fresh ruos disk on `dev`: a GPT (ESP of `esp_mib` MiB + a data
/// partition filling the rest) with both partitions FAT32-formatted and
/// `/EFI/BOOT` created on the ESP. **Destructive** — overwrites the device.
/// Returns the partition layout.
///
/// Each partition is operated on through a `PartBorrow` whose lifetime ends
/// before the next one begins (the borrows are scoped), so only one partition
/// view of `dev` is live at a time.
pub fn author(dev: &mut dyn BlockDevice, esp_mib: u32) -> Result<Layout, DiskError> {
    let esp_sectors = (esp_mib as u64) * 1024 * 1024 / 512;
    let (esp, data) =
        crate::gpt::write_layout(dev, esp_sectors).map_err(|_| DiskError::TooSmall)?;

    // ESP: format + create the EFI boot dir tree.
    {
        let mut e = PartBorrow::new(dev, esp.first_lba, esp.sectors);
        crate::vfs::fat32::format(&mut e).map_err(|_| DiskError::Io)?;
        crate::vfs::fat32::create_dirs(&mut e, &["/EFI", "/EFI/BOOT"])
            .map_err(|_| DiskError::Io)?;
    }

    // Data partition: just a fresh FAT32 for now.
    {
        let mut d = PartBorrow::new(dev, data.first_lba, data.sectors);
        crate::vfs::fat32::format(&mut d).map_err(|_| DiskError::Io)?;
    }

    Ok(Layout { esp, data })
}

/// Loose Limine modules che restano sull'ESP come bootstrap (init chain). NB:
/// `shell.wasm` NON è più un modulo loose — vive dentro `bin.bgz` (membro
/// dell'archivio) e viene estratto sull'ESP a parte (vedi `copy_boot_payload`).
const ESP_BOOTSTRAP: &[&str] = &["/init.wasm", "/etc/init.sh"];

/// Nome del membro di `bin.bgz` da estrarre sull'ESP per il bootstrap: la slim
/// limine.conf lo module-carica come `/bin/shell.wasm` prima che `/mnt` sia su.
const ESP_SHELL_MEMBER: &str = "shell.wasm";

/// Write the boot tree onto a freshly-authored disk: the bootstrap (+ the slim
/// limine.conf) to the ESP, the command-line tools to the data partition.
///
/// The ESP gets BOOTX64.EFI + kernel + the SLIM limine.conf (which becomes the
/// SSD's `/boot/limine/limine.conf`) + the loose bootstrap modules (init chain)
/// + `shell.wasm` estratto da `bin.bgz`, so the SSD boots standalone
/// (UEFI → /EFI/BOOT/BOOTX64.EFI → limine.conf → kernel + init + shell).
///
/// I tool live NON sono più moduli loose: sono impacchettati in `bin.bgz`
/// (`/archive/`), che sull'ISO live viene scompattato in tmpfs `/bin` dalla fase
/// `unpack_bin`. Qui replichiamo lo stesso /bin sul disco: scompattiamo OGNI
/// membro di `bin.bgz` direttamente sulla data partition (`/bin/<name>`), dove
/// monta a `/mnt/bin` e carica on-demand. `author` ha già FAT32-formattato
/// ENTRAMBE le partizioni e creato /EFI/BOOT sull'ESP; `write_file` crea `/bin`
/// (e ogni dir intermedia) on demand.
pub fn copy_boot_payload(dev: &mut dyn crate::blockdev::BlockDevice,
                         layout: &Layout) -> Result<(), DiskError> {
    use crate::vfs::fat32::FatWriter;
    use crate::blockdev::PartBorrow;
    use gzip_core::pack;

    // L'archivio /bin (bin.bgz) è la sorgente di TUTTI i tool (incluso shell).
    let archive = crate::modules::archive("bin.bgz").ok_or(DiskError::Io)?;

    // --- ESP: BOOTX64.EFI + kernel + slim limine.conf + bootstrap loose + shell ---
    {
        let mut esp = PartBorrow::new(dev, layout.esp.first_lba, layout.esp.sectors);
        let mut w = FatWriter::open(&mut esp).map_err(|_| DiskError::Io)?;
        w.write_file("/EFI/BOOT/BOOTX64.EFI",
            crate::modules::payload("BOOTX64.EFI").ok_or(DiskError::Io)?)
            .map_err(|_| DiskError::Io)?;
        w.write_file("/boot/kernel",
            crate::modules::payload("kernel").ok_or(DiskError::Io)?)
            .map_err(|_| DiskError::Io)?;
        // the SLIM config becomes the SSD's limine.conf:
        w.write_file("/boot/limine/limine.conf",
            crate::modules::payload("limine-ssd.conf").ok_or(DiskError::Io)?)
            .map_err(|_| DiskError::Io)?;
        // init chain (moduli loose):
        for (cmdline, data) in crate::modules::all() {
            if ESP_BOOTSTRAP.contains(&cmdline) {
                w.write_file(cmdline, data).map_err(|_| DiskError::Io)?;
            }
        }
        // shell.wasm: estrai il membro da bin.bgz → ESP /bin/shell.wasm (la slim
        // limine.conf lo module-carica prima che /mnt sia montata).
        let shell = extract_member(archive, ESP_SHELL_MEMBER)?;
        w.write_file("/bin/shell.wasm", &shell).map_err(|_| DiskError::Io)?;
    } // esp PartBorrow dropped — releases the &mut dev borrow

    // --- DATA partition: scompatta TUTTO bin.bgz → /bin (mount at /mnt/bin) ---
    {
        let mut d = PartBorrow::new(dev, layout.data.first_lba, layout.data.sectors);
        let mut w = FatWriter::open(&mut d).map_err(|_| DiskError::Io)?;
        let iter = pack::parse(archive).map_err(|_| DiskError::Io)?;
        for entry in iter {
            let (name, gz) = entry.map_err(|_| DiskError::Io)?;
            // Skip-on-OOM (come unpack_bin): un membro gigante (app .cwasm da
            // decine di MB) non deve uccidere l'install per heap frammentato.
            // L'app mancherà da /mnt/bin ma il disco resta installabile.
            let need = isize_of(gz);
            if !heap_has(need) {
                crate::bwarn!("install",
                    "{}: skipped — no contiguous {} MiB of heap for inflate", name, need >> 20);
                continue;
            }
            let data = match pack::decompress_member(gz) {
                Ok(d) => d,
                Err(_) => { crate::bwarn!("install", "{}: inflate failed, skipped", name); continue; }
            };
            let path = alloc::format!("/bin/{}", name);
            w.write_file(&path, &data).map_err(|_| DiskError::Io)?; // shell.wasm → data:/bin/shell.wasm, ls.wasm → ...
        }
    }
    Ok(())
}

/// Estrai e decomprimi un membro per nome da un archivio `bin.bgz`. Usato per
/// portare `shell.wasm` sull'ESP (bootstrap). `Err(Io)` se assente/corrotto.
fn extract_member(archive: &[u8], name: &str) -> Result<alloc::vec::Vec<u8>, DiskError> {
    use gzip_core::pack;
    for entry in pack::parse(archive).map_err(|_| DiskError::Io)? {
        let (n, gz) = entry.map_err(|_| DiskError::Io)?;
        if n == name {
            return pack::decompress_member(gz).map_err(|_| DiskError::Io);
        }
    }
    Err(DiskError::Io)
}

/// Uncompressed size of a gzip member (ISIZE, ultimi 4 byte LE); 0 se malformato.
fn isize_of(gz: &[u8]) -> usize {
    match gz.len().checked_sub(4) {
        Some(i) => u32::from_le_bytes([gz[i], gz[i + 1], gz[i + 2], gz[i + 3]]) as usize,
        None => 0,
    }
}

/// `true` se l'heap ha un blocco contiguo da `bytes` ORA (probe con
/// `try_reserve_exact`, rilasciato subito). Vedi `unpack_bin::heap_has`.
fn heap_has(bytes: usize) -> bool {
    if bytes == 0 { return true; }
    let mut probe: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    probe.try_reserve_exact(bytes).is_ok()
}

// ─── Session API ─────────────────────────────────────────────────────────────
// Stepped install: ogni chiamata a `session_step` copia UN file e torna il
// controllo al tool WASM, che può aggiornare barra di avanzamento prima della
// prossima chiamata.

/// File fissi scritti sull'ESP nella fase bootstrap (ordine esatto).
const N_ESP: usize = 6;
const ESP_PATHS: [&str; N_ESP] = [
    "/EFI/BOOT/BOOTX64.EFI",
    "/boot/kernel",
    "/boot/limine/limine.conf",
    "/init.wasm",
    "/etc/init.sh",
    "/bin/shell.wasm",
];
const ESP_NAMES: [&str; N_ESP] = [
    "BOOTX64.EFI", "kernel", "limine.conf", "init.wasm", "init.sh", "shell.wasm",
];

/// Stato di un'installazione in corso. Mantenuto tra le chiamate a
/// `session_step`; va conservato in un `static Mutex<Option<InstallSession>>`
/// nel layer host fn. `AhciPort` è `Send`, quindi sicuro in un Mutex globale.
pub struct InstallSession {
    pub port:   crate::ahci::AhciPort,
    pub layout: Layout,
    /// Membri di `bin.bgz` (nome, gz raw); slice `'static` (HHDM, kernel lifetime).
    pub members: alloc::vec::Vec<(&'static str, &'static [u8])>,
    /// Numero totale di step (N_ESP + members.len()).
    pub total: usize,
    /// Prossimo step da eseguire (0-indexed). Quando == total, l'install è finito.
    pub step: usize,
}

/// Apre una nuova sessione di installazione: GPT + FAT, raccolta membri archivio.
/// Non controlla il guard `/mnt` (spetta al caller / host fn).
pub fn session_open(port_idx: usize, esp_mib: u32) -> Result<InstallSession, DiskError> {
    let mut port = crate::ahci::acquire_port(port_idx).ok_or(DiskError::Io)?;
    crate::binfo!("install",
        "target port {} model={:?} sectors={} ({} MiB) — WIPING",
        port_idx, port.model, port.sectors, port.sectors / 2048);
    let layout = author(&mut port, esp_mib)?;
    let members = collect_members()?;
    let total = N_ESP + members.len();
    Ok(InstallSession { port, layout, members, total, step: 0 })
}

/// Esegue il prossimo step dell'installazione: copia UN file sull'ESP o sulla
/// data partition. Al completamento dell'ultimo file esegue anche il flush.
///
/// Ritorna `(done_count, display_name)` dove `done_count` è 1-indexed; quando
/// `done_count == session.total` l'installazione è completa. `Err(Io)` su fallimento.
pub fn session_step(s: &mut InstallSession) -> Result<(usize, &'static str), DiskError> {
    let idx = s.step;
    if idx >= s.total { return Err(DiskError::Io); }
    let name: &'static str = if idx < N_ESP { ESP_NAMES[idx] } else { s.members[idx - N_ESP].0 };
    if idx < N_ESP {
        esp_step_single(&mut s.port, &s.layout, idx)?;
    } else {
        let (mname, gz) = s.members[idx - N_ESP];
        data_step_single(&mut s.port, &s.layout, mname, gz)?;
    }
    s.step += 1;
    if s.step == s.total {
        if s.port.flush().is_err() {
            crate::bwarn!("install", "cache flush failed (writes may not be durable)");
        }
        crate::binfo!("install", "ok — {} files written, disk flushed", s.total);
    }
    Ok((s.step, name))
}

/// Raccoglie tutti i membri di `bin.bgz` in un Vec per accesso per indice.
/// I puntatori sono `'static` perché l'archivio risiede nell'HHDM.
fn collect_members() -> Result<alloc::vec::Vec<(&'static str, &'static [u8])>, DiskError> {
    use gzip_core::pack;
    let archive = crate::modules::archive("bin.bgz").ok_or(DiskError::Io)?;
    let mut out = alloc::vec::Vec::new();
    for entry in pack::parse(archive).map_err(|_| DiskError::Io)? {
        let (name, gz) = entry.map_err(|_| DiskError::Io)?;
        // SAFETY: `archive` è &'static [u8] (HHDM, kernel lifetime);
        // le slice restituite dall'iterator condividono quella lifetime.
        let name: &'static str = unsafe { core::mem::transmute(name) };
        let gz:   &'static [u8] = unsafe { core::mem::transmute(gz)   };
        out.push((name, gz));
    }
    Ok(out)
}

/// Copia UN file sull'ESP (indice 0..N_ESP). Apre FatWriter fresh ogni volta —
/// sicuro perché FatWriter scrive su disco a ogni `write_sector` (nessun dirty
/// cache in RAM tra call separate).
fn esp_step_single(port: &mut crate::ahci::AhciPort, layout: &Layout, idx: usize) -> Result<(), DiskError> {
    use crate::vfs::fat32::FatWriter;
    let mut esp = PartBorrow::new(port, layout.esp.first_lba, layout.esp.sectors);
    let mut w = FatWriter::open(&mut esp).map_err(|_| DiskError::Io)?;
    match idx {
        0 => w.write_file("/EFI/BOOT/BOOTX64.EFI",
                crate::modules::payload("BOOTX64.EFI").ok_or(DiskError::Io)?)
             .map_err(|_| DiskError::Io),
        1 => w.write_file("/boot/kernel",
                crate::modules::payload("kernel").ok_or(DiskError::Io)?)
             .map_err(|_| DiskError::Io),
        2 => w.write_file("/boot/limine/limine.conf",
                crate::modules::payload("limine-ssd.conf").ok_or(DiskError::Io)?)
             .map_err(|_| DiskError::Io),
        3 => { let d = find_module("/init.wasm").ok_or(DiskError::Io)?;
               w.write_file("/init.wasm", d).map_err(|_| DiskError::Io) }
        4 => { let d = find_module("/etc/init.sh").ok_or(DiskError::Io)?;
               w.write_file("/etc/init.sh", d).map_err(|_| DiskError::Io) }
        5 => {
            let archive = crate::modules::archive("bin.bgz").ok_or(DiskError::Io)?;
            let shell = extract_member(archive, "shell.wasm")?;
            w.write_file("/bin/shell.wasm", &shell).map_err(|_| DiskError::Io)
        }
        _ => Err(DiskError::Io),
    }
}

/// Decomprime un membro `bin.bgz` e lo scrive sulla data partition. Skip-on-OOM.
fn data_step_single(port: &mut crate::ahci::AhciPort, layout: &Layout,
                    name: &str, gz: &[u8]) -> Result<(), DiskError> {
    use gzip_core::pack;
    use crate::vfs::fat32::FatWriter;
    let need = isize_of(gz);
    if !heap_has(need) {
        crate::bwarn!("install", "{}: skipped (no contiguous {} MiB heap)", name, need >> 20);
        return Ok(()); // skip, non errore
    }
    let data = match pack::decompress_member(gz) {
        Ok(d) => d,
        Err(_) => { crate::bwarn!("install", "{}: inflate failed, skipped", name); return Ok(()); }
    };
    let path = alloc::format!("/bin/{}", name);
    let mut d = PartBorrow::new(port, layout.data.first_lba, layout.data.sectors);
    let mut w = FatWriter::open(&mut d).map_err(|_| DiskError::Io)?;
    w.write_file(&path, &data).map_err(|_| DiskError::Io)
}

/// Trova un modulo loose Limine per cmdline path. `None` se non caricato.
fn find_module(path: &str) -> Option<&'static [u8]> {
    crate::modules::all().into_iter().find(|(p, _)| *p == path).map(|(_, d)| d)
}
