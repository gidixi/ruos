//! Phase 5 — filesystem: VFS init + Limine modules mount.

use crate::boot::BootError;

pub fn init() -> Result<(), BootError> {
    let n = crate::vfs::init()
        .map_err(|_| BootError::VfsInit("vfs init failed"))?;
    crate::binfo!("fs", "vfs init ok mounts={}", n);
    crate::pty::init();

    #[cfg(feature = "boot-checks")]
    {
        let result = crate::vfs::block_on(async {
            use crate::vfs::{open, write, read, seek, close, OpenFlags, Whence};
            // /dev/null write smoke.
            let fd = open("/dev/null", OpenFlags::WRITE).await?;
            write(fd, b"hello").await?;
            close(fd).await?;
            // /tmp/x: create, write, seek, read back.
            let fd = open(
                "/tmp/x",
                OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::READ,
            ).await?;
            write(fd, b"abc").await?;
            seek(fd, 0, Whence::Set).await?;
            let mut buf = [0u8; 8];
            let n = read(fd, &mut buf).await?;
            close(fd).await?;
            Ok::<(usize, [u8; 8]), crate::vfs::VfsError>((n, buf))
        });
        match result {
            Ok((n, buf)) => crate::binfo!(
                "fs",
                "vfs smoke ok n={} buf=[{}]",
                n,
                core::str::from_utf8(&buf[..n]).unwrap_or("?"),
            ),
            Err(e) => return Err(BootError::VfsInit(
                // VfsError doesn't map to &'static str easily, use a generic message
                match e {
                    _ => "vfs smoke failed",
                }
            )),
        }
    }

    // I bin NON vengono più montati qui dai moduli Limine: la fase `storage`
    // monta `/bin` dal CD live (ISO9660/ATAPI) e, SOLO se non c'è CD, fa il
    // fallback a `modules::mount_all()`. Lasciamo intatti i `/dev` e la root.
    crate::binfo!("fs", "bin mount deferred to storage phase");

    #[cfg(feature = "boot-checks")]
    {
        // Real `cat` over the WASI file path (needs the VFS, hence this phase).
        let cc = crate::wasm::wt::run_cat_demo();
        crate::binfo!("wt", "wasmtime WASI cat exit={}", cc);
        // gfx service: direct blit + pixel readback.
        let g = crate::gfx::geom();
        crate::binfo!("gfx", "geom w={} h={} stride={} fmt={}", g.width, g.height, g.stride, g.format);
        let gok = crate::gfx::self_test();
        crate::binfo!("gfx", "gfx blit self-test {} (count={} last=0x{:08X})",
            if gok { "ok" } else { "FAIL" }, crate::gfx::blit_count(), crate::gfx::last_pixel());
        // gfx host-fn path: a .cwasm calling gfx_info + gfx_blit (red).
        let _ = crate::wasm::wt::run_gfxtest_demo();
        let hok = crate::gfx::blit_count() > 0 && crate::gfx::last_pixel() == 0xFF0000FF;
        crate::binfo!("gfx", "gfx host self-test {}", if hok { "ok" } else { "FAIL" });
    }

    Ok(())
}
