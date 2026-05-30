//! Ed25519 host-key load/generate.
//!
//! Storage format: raw 32-byte seed. We persist only the seed; the matching
//! `VerifyingKey` is re-derived from it at boot.
//!
//! On first boot the file doesn't exist → we draw 32 bytes from the kernel
//! CSPRNG (`crate::rng::fill`), write them out, and log `ssh: host key
//! generated`. Subsequent boots load the seed and log `ssh: host key loaded`.

use alloc::vec;
use alloc::vec::Vec;
use ed25519_dalek::SigningKey;

use crate::ssh::SshError;
use crate::vfs::{block_on, OpenFlags};

pub struct HostKey {
    pub signing: SigningKey,
}

impl HostKey {
    /// 32-byte raw public key (matches OpenSSH `ssh-ed25519` wire format).
    pub fn public(&self) -> [u8; 32] {
        self.signing.verifying_key().to_bytes()
    }
}

pub fn load_or_generate(path: &str) -> Result<HostKey, SshError> {
    if let Some(seed) = try_load(path) {
        crate::binfo!("ssh", "host key loaded ({} bytes seed)", seed.len());
        let arr: [u8; 32] = seed.try_into().map_err(|_| SshError::Crypto)?;
        return Ok(HostKey { signing: SigningKey::from_bytes(&arr) });
    }
    let mut seed = [0u8; 32];
    crate::rng::fill(&mut seed);
    write_new(path, &seed)?;
    crate::binfo!("ssh", "host key generated at {}", path);
    Ok(HostKey { signing: SigningKey::from_bytes(&seed) })
}

fn try_load(path: &str) -> Option<Vec<u8>> {
    let fd = match block_on(crate::vfs::open(path, OpenFlags::READ)) {
        Ok(fd) => fd,
        Err(_) => return None,
    };
    let mut buf = vec![0u8; 32];
    let n = block_on(crate::vfs::read(fd, &mut buf)).ok()?;
    let _ = block_on(crate::vfs::close(fd));
    if n < 32 { return None; }
    Some(buf)
}

fn write_new(path: &str, seed: &[u8; 32]) -> Result<(), SshError> {
    // Path is a top-level file on /mnt (FAT root) so no parent mkdir needed.
    let fd = block_on(crate::vfs::open(path,
        OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE))
        .map_err(|e| {
            crate::bwarn!("ssh", "open {} for write failed: {}", path, e);
            SshError::VfsIo
        })?;
    let n = block_on(crate::vfs::write(fd, seed))
        .map_err(|_| SshError::VfsIo)?;
    let _ = block_on(crate::vfs::close(fd));
    if n != 32 { return Err(SshError::VfsIo); }
    Ok(())
}
