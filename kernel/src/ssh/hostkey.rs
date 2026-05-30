//! Ed25519 host-key load/generate.
//!
//! Storage format: raw 32-byte seed (NOT OpenSSH PEM). Trivial parsing.
//! The matching public key is re-derived from the seed each boot.
//!
//! Task 2 will replace the stub with real `ed25519-dalek` integration.

use crate::ssh::SshError;

/// Currently a stub — Task 2 populates this.
pub fn load_or_generate(_path: &str) -> Result<[u8; 32], SshError> {
    Err(SshError::NotImplemented)
}
