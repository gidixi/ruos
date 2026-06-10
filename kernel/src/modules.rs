//! Limine boot modules → VFS mount.
//!
//! At boot, Limine loads N modules declared in `limine.conf`. Each
//! module has a virtual path (its `module_cmdline`) and an in-RAM
//! buffer (mapped in HHDM). `mount_all()` copies each module into
//! the existing tmpfs at its declared path, so userspace .wasm files
//! become regular VFS entries (`/init.wasm`, `/server.wasm`, ...).
//!
//! Modules whose cmdline is under `/payload/` are NOT tmpfs-mounted:
//! they are the boot artifacts (kernel ELF, BOOTX64.EFI, limine.conf)
//! shipped so the running kernel can read their bytes back and copy
//! them onto an SSD ESP. The kernel ELF alone is multi-MB; tmpfs is
//! heap-backed, so we leave these in their HHDM module buffers and
//! expose them lazily via `payload()` / `all()`.

use crate::vfs;
use crate::vfs::OpenFlags;
use limine::request::ModulesRequest;

/// Module cmdline prefix marking a boot artifact (see module docs).
const PAYLOAD_PREFIX: &str = "/payload/";

/// Module cmdline prefix per l'archivio /bin compresso (bin.bgz). Skippato dal
/// tmpfs-mount come `/payload/`; recuperato via `archive()`.
const ARCHIVE_PREFIX: &str = "/archive/";

/// Module cmdline prefix per il set rescue (shell + tool minimi). Tenuto in
/// HHDM, scritto in /bin SOLO se l'unpack di bin.bgz fallisce (`rescue_all()`).
const RESCUE_PREFIX: &str = "/rescue/";

#[used]
#[link_section = ".requests"]
static MODULES: ModulesRequest = ModulesRequest::new();

/// Iterate Limine modules, create+write each in tmpfs. `/payload/`
/// modules are skipped (left in their HHDM buffers for `payload()`).
/// Returns the count actually mounted into tmpfs.
pub fn mount_all() -> usize {
    let Some(resp) = MODULES.response() else {
        crate::bwarn!("mod", "no Limine modules response");
        return 0;
    };
    let mods = resp.modules();
    let mut mounted = 0usize;
    let mut payloads = 0usize;
    for m in mods {
        // m.data() is the HHDM-mapped module buffer (guaranteed valid for
        // kernel lifetime). m.cmdline() is the path declared in limine.conf.
        let path = m.cmdline();
        if path.starts_with(PAYLOAD_PREFIX)
            || path.starts_with(ARCHIVE_PREFIX)
            || path.starts_with(RESCUE_PREFIX)
        {
            // Boot artifact / archivio / rescue — non file tmpfs diretti.
            payloads += 1;
            continue;
        }
        let bytes: &[u8] = m.data();
        match install(path, bytes) {
            Ok(()) => {
                mounted += 1;
                crate::binfo!("mod", "mounted {} ({} bytes)", path, bytes.len());
            }
            Err(e) => {
                crate::bwarn!("mod", "install fail {}: {:?}", path, e);
            }
        }
    }
    crate::binfo!(
        "mod",
        "mounted {} boot modules ({} payload skipped)",
        mounted,
        payloads
    );
    log_payload_sizes();
    mounted
}

/// One-time serial line confirming the three boot artifacts were loaded
/// by Limine (helps verify this task and feeds copy_boot_payload, Task 3).
fn log_payload_sizes() {
    let k = payload("kernel").map_or(0, <[u8]>::len);
    let b = payload("BOOTX64.EFI").map_or(0, <[u8]>::len);
    let c = payload("limine.conf").map_or(0, <[u8]>::len);
    let s = payload("limine-ssd.conf").map_or(0, <[u8]>::len);
    crate::binfo!(
        "mod",
        "payload: kernel={}B bootx64={}B conf={}B ssdconf={}B",
        k,
        b,
        c,
        s
    );
}

fn install(path: &str, bytes: &[u8]) -> Result<(), vfs::VfsError> {
    vfs::block_on(async {
        let fd = vfs::open(path, OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::READ).await?;
        vfs::write(fd, bytes).await?;
        vfs::close(fd).await?;
        Ok(())
    })
}

/// Bytes of a `/payload/<name>` Limine module (kernel / BOOTX64.EFI /
/// limine.conf). The module buffer is HHDM-mapped and never freed, and
/// `MODULES` is a `static`, so the slice is valid for the kernel lifetime.
pub fn payload(name: &str) -> Option<&'static [u8]> {
    let resp = MODULES.response()?;
    for m in resp.modules() {
        let cmdline = m.cmdline();
        if let Some(stripped) = cmdline.strip_prefix(PAYLOAD_PREFIX) {
            if stripped == name {
                // SAFETY: `m.data()` borrows the HHDM-mapped module buffer.
                // That buffer lives for the whole kernel lifetime (Limine
                // never reclaims loaded modules and `MODULES` is `static`),
                // so widening the borrow to `'static` is sound.
                return Some(unsafe { core::mem::transmute::<&[u8], &'static [u8]>(m.data()) });
            }
        }
    }
    None
}

/// Every boot module as `(cmdline, data)` — used by copy_boot_payload
/// (Task 3) to walk the `/payload/` artifacts. Both the cmdline string
/// and the data slice are valid for the kernel lifetime (see `payload`).
pub fn all() -> alloc::vec::Vec<(&'static str, &'static [u8])> {
    let mut out = alloc::vec::Vec::new();
    if let Some(resp) = MODULES.response() {
        for m in resp.modules() {
            // SAFETY: see `payload()` — the module buffer and cmdline string
            // (a NUL-terminated C string in the Limine response) both live
            // for the kernel lifetime; widen the borrows to `'static`.
            let cmdline = unsafe { core::mem::transmute::<&str, &'static str>(m.cmdline()) };
            let data = unsafe { core::mem::transmute::<&[u8], &'static [u8]>(m.data()) };
            out.push((cmdline, data));
        }
    }
    out
}

/// Bytes del modulo archivio `/archive/<name>` (es. `bin.bgz`), HHDM-mapped e
/// valido per tutta la vita kernel (vedi `payload`). `None` se assente.
pub fn archive(name: &str) -> Option<&'static [u8]> {
    let resp = MODULES.response()?;
    for m in resp.modules() {
        if let Some(stripped) = m.cmdline().strip_prefix(ARCHIVE_PREFIX) {
            if stripped == name {
                // SAFETY: `m.data()` borrows the HHDM-mapped module buffer.
                // That buffer lives for the whole kernel lifetime (Limine
                // never reclaims loaded modules and `MODULES` is `static`),
                // so widening the borrow to `'static` is sound.
                return Some(unsafe { core::mem::transmute::<&[u8], &'static [u8]>(m.data()) });
            }
        }
    }
    None
}

/// Tutti i moduli rescue come `(basename, data)` (es. `("shell.wasm", &[..])`).
/// Usati per popolare `/bin` quando bin.bgz manca/è corrotto.
pub fn rescue_all() -> alloc::vec::Vec<(&'static str, &'static [u8])> {
    let mut out = alloc::vec::Vec::new();
    if let Some(resp) = MODULES.response() {
        for m in resp.modules() {
            if let Some(name) = m.cmdline().strip_prefix(RESCUE_PREFIX) {
                // SAFETY: see `payload()` — the module buffer and cmdline string
                // both live for the kernel lifetime; widen the borrows to `'static`.
                let name = unsafe { core::mem::transmute::<&str, &'static str>(name) };
                let data = unsafe { core::mem::transmute::<&[u8], &'static [u8]>(m.data()) };
                out.push((name, data));
            }
        }
    }
    out
}
