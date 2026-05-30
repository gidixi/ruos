//! authorized_keys reader.
//!
//! OpenSSH format, one entry per line:
//!   `ssh-ed25519 <base64-encoded-32-byte-pubkey> [comment]`
//!
//! Task 3 populates the real parser + inline base64 decoder.

use alloc::vec::Vec;
use crate::ssh::SshError;

/// Currently a stub — Task 3 populates this.
pub fn load(_path: &str) -> Result<Vec<[u8; 32]>, SshError> {
    Err(SshError::NotImplemented)
}
