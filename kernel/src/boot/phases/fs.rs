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

    crate::modules::mount_all();
    crate::binfo!("fs", "modules mounted");

    Ok(())
}
