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
            crate::bwarn!("unpack_bin", "bin.bgz module missing → rescue fallback");
            rescue_fallback();
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
            Ok((name, gz)) => match pack::decompress_member(gz) {
                Ok(data) => {
                    let path = format!("/bin/{}", name);
                    if write_file(&path, &data).is_ok() {
                        ok += 1;
                    } else {
                        fail += 1;
                        crate::bwarn!("unpack_bin", "write {} failed", name);
                    }
                }
                Err(e) => {
                    fail += 1;
                    crate::bwarn!("unpack_bin", "{}: {}", name, e);
                }
            },
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
    for (name, data) in crate::modules::rescue_all() {
        let path = format!("/bin/{}", name);
        if write_file(&path, data).is_ok() {
            n += 1;
        }
    }
    if n == 0 {
        panic!("unpack_bin: bin.bgz unusable AND no /rescue/ modules — system has no /bin");
    }
    crate::binfo!("unpack_bin", "rescue: {} fallback bins in /bin", n);
}

fn write_file(path: &str, bytes: &[u8]) -> Result<(), vfs::VfsError> {
    vfs::block_on(async {
        let fd = vfs::open(path, OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::READ).await?;
        vfs::write(fd, bytes).await?;
        vfs::close(fd).await?;
        Ok(())
    })
}
