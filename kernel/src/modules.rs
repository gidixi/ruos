//! Limine boot modules → VFS mount.
//!
//! At boot, Limine loads N modules declared in `limine.conf`. Each
//! module has a virtual path (its `module_cmdline`) and an in-RAM
//! buffer (mapped in HHDM). `mount_all()` copies each module into
//! the existing tmpfs at its declared path, so userspace .wasm files
//! become regular VFS entries (`/init.wasm`, `/server.wasm`, ...).

use crate::vfs;
use crate::vfs::OpenFlags;
use limine::request::ModulesRequest;

#[used]
#[link_section = ".requests"]
static MODULES: ModulesRequest = ModulesRequest::new();

/// Iterate Limine modules, create+write each in tmpfs. Returns count
/// mounted.
pub fn mount_all() -> usize {
    let Some(resp) = MODULES.response() else {
        crate::bwarn!("mod", "no Limine modules response");
        return 0;
    };
    let mods = resp.modules();
    for m in mods {
        // m.data() is the HHDM-mapped module buffer (guaranteed valid for
        // kernel lifetime). m.cmdline() is the path declared in limine.conf.
        let bytes: &[u8] = m.data();
        let path = m.cmdline();
        match install(path, bytes) {
            Ok(()) => {
                crate::binfo!("mod", "mounted {} ({} bytes)", path, bytes.len());
            }
            Err(e) => {
                crate::bwarn!("mod", "install fail {}: {:?}", path, e);
            }
        }
    }
    crate::binfo!("mod", "mounted {} boot modules", mods.len());
    mods.len()
}

fn install(path: &str, bytes: &[u8]) -> Result<(), vfs::VfsError> {
    vfs::block_on(async {
        let fd = vfs::open(path, OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::READ).await?;
        vfs::write(fd, bytes).await?;
        vfs::close(fd).await?;
        Ok(())
    })
}
