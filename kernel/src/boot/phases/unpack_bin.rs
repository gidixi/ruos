//! Phase — unpack_bin: popola `/bin` decomprimendo l'archivio `bin.bgz`.
//!
//! `bin.bgz` è un modulo Limine (`/archive/bin.bgz`) caricato in RAM (HHDM) dal
//! firmware UEFI: leggibile su ogni HW, indipendente da driver USB-MSC/ATAPI.
//! Qui lo parsiamo (container RBIN) e scriviamo ogni membro gzip decompresso in
//! tmpfs `/bin`. Se l'archivio manca/è corrotto → set rescue dai moduli
//! `/rescue/`. La chiavetta USB è scollegabile appena finita questa fase.

use crate::boot::BootError;
use crate::vfs::{self, OpenFlags};
use alloc::format;
use gzip_core::pack;

pub fn init() -> Result<(), BootError> {
    match crate::modules::archive("bin.bgz") {
        Some(bytes) => unpack(bytes),
        None => {
            // Nessun bin.bgz. Due casi:
            //  - ISO live → l'archivio dovrebbe esserci: assenza = rescue fallback.
            //  - SSD installato → la slim limine.conf NON carica bin.bgz di proposito:
            //    i tool stanno sulla data partition, montata a /mnt/bin dalla fase
            //    storage (che gira DOPO questa). Qui /bin ha solo shell.wasm (modulo
            //    ESP); NON è un errore → WARN morbido, niente panic.
            // Distinguo col set rescue: presente solo sull'ISO live. Se manca anche
            // quello, siamo su SSD → continua; init userà /mnt/bin.
            if crate::modules::rescue_all().is_empty() {
                crate::bwarn!("unpack_bin",
                    "no bin.bgz and no /rescue/ — installed SSD boot, tools come from /mnt/bin");
            } else {
                crate::bwarn!("unpack_bin", "bin.bgz module missing → rescue fallback");
                rescue_fallback();
            }
        }
    }
    Ok(())
}

fn unpack(bytes: &[u8]) {
    let iter = match pack::parse(bytes) {
        Ok(it) => it,
        Err(e) => {
            crate::bwarn!("unpack_bin", "bin.bgz parse error {:?} → rescue fallback", e);
            rescue_fallback();
            return;
        }
    };
    let mut ok = 0usize;
    let mut fail = 0usize;
    for entry in iter {
        match entry {
            Ok((name, gz)) => {
                // Skip-on-OOM: un membro gigante (app .cwasm da decine di MB,
                // drop-folder apps/) non deve MAI uccidere il boot. L'alloc di
                // `decompress_member` (presize ISIZE) e la copia in tmpfs sono
                // infallibili (panic su OOM) → sondiamo PRIMA con try_reserve:
                // se l'heap non ha un blocco contiguo da ISIZE byte, salta il
                // membro con WARN e continua (l'app mancherà da /bin, il
                // sistema boota).
                let need = isize_of(gz);
                if !heap_has(need) {
                    fail += 1;
                    crate::bwarn!("unpack_bin",
                        "{}: skipped — no contiguous {} MiB of heap for inflate", name, need >> 20);
                    continue;
                }
                match pack::decompress_member(gz) {
                    Ok(data) => {
                        // MOVE the inflate buffer straight into tmpfs: the heap
                        // holds the blob ONCE, not twice. A buffered write copied
                        // `data` into a second contiguous Vec while the first was
                        // still alive (2× peak) — fatal for a big app `.cwasm`
                        // (~74 MiB viewer) that won't fit twice in the 384 MiB heap.
                        let path = format!("/bin/{}", name);
                        let n = data.len();
                        match crate::vfs::write_file_owned(&path, data) {
                            Ok(()) => ok += 1,
                            Err(e) => {
                                fail += 1;
                                crate::bwarn!("unpack_bin",
                                    "write {} ({} MiB) failed: {:?}", name, n >> 20, e);
                            }
                        }
                    }
                    Err(e) => {
                        fail += 1;
                        crate::bwarn!("unpack_bin", "{}: {}", name, e);
                    }
                }
            }
            Err(e) => {
                fail += 1;
                crate::bwarn!("unpack_bin", "archive entry error {:?}", e);
            }
        }
    }
    if ok == 0 {
        crate::bwarn!("unpack_bin", "no bins unpacked → rescue fallback");
        rescue_fallback();
        return;
    }
    crate::binfo!("unpack_bin", "unpacked {} bins from bin.bgz ({} failed)", ok, fail);
}

fn rescue_fallback() {
    let mut n = 0usize;
    let mut fail = 0usize;
    for (name, data) in crate::modules::rescue_all() {
        let path = format!("/bin/{}", name);
        if write_file(&path, data).is_ok() {
            n += 1;
        } else {
            fail += 1;
            crate::bwarn!("unpack_bin", "rescue write {} failed", name);
        }
    }
    if n == 0 {
        panic!("unpack_bin: bin.bgz unusable AND no /rescue/ modules — system has no /bin");
    }
    crate::binfo!("unpack_bin", "rescue: {} fallback bins in /bin ({} failed)", n, fail);
}

/// Uncompressed size of a gzip member (ISIZE, ultimi 4 byte little-endian).
/// 0 su membri malformati — il probe da 0 byte passa e l'errore emerge poi
/// dal normale path di decompressione.
fn isize_of(gz: &[u8]) -> usize {
    match gz.len().checked_sub(4) {
        Some(i) => u32::from_le_bytes([gz[i], gz[i + 1], gz[i + 2], gz[i + 3]]) as usize,
        None => 0,
    }
}

/// `true` se l'heap ha un blocco contiguo da `bytes` disponibile ORA. Sonda
/// con `try_reserve_exact` su un Vec vuoto e rilascia subito: l'alloc vera
/// (stessa size, subito dopo, fase single-threaded) riottiene quel blocco.
fn heap_has(bytes: usize) -> bool {
    if bytes == 0 { return true; }
    let mut probe: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    probe.try_reserve_exact(bytes).is_ok()
}

fn write_file(path: &str, bytes: &[u8]) -> Result<(), vfs::VfsError> {
    vfs::block_on(async {
        let fd = vfs::open(path, OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::READ).await?;
        vfs::write(fd, bytes).await?;
        vfs::close(fd).await?;
        Ok(())
    })
}
