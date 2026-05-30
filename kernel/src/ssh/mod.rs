//! SSH server (Step 16).
//!
//! Public entry: [`spawn`]. Called once at boot from
//! `boot::phases::userland::init` after `net::init`. Non-fatal: if any
//! prerequisite (FAT mount, networking, RNG, host key) is missing, the
//! spawn logs a warning and the kernel continues without SSH.

pub mod authkeys;
pub mod channel;
pub mod hostkey;
pub mod rng_bridge;
pub mod server;
pub mod sunset_io;

use core::fmt;

/// Server config. Static for now — the host key and authorized_keys paths
/// match the convention from the design spec (`/mnt/etc/ssh/`).
pub struct Config {
    pub port:          u16,
    pub host_key_path: &'static str,
    pub authkeys_path: &'static str,
}

pub static CONFIG: Config = Config {
    port:          22,
    // Top-level on /mnt for now — our FAT32 driver doesn't yet support
    // mkdir, and pre-populating /etc/ssh/ in disk.img would add steps to
    // the Makefile. Names kept inside the FAT 8.3 short-name limit
    // (8-char base + 3-char ext) until LFN support lands.
    host_key_path: "/mnt/host.key",
    authkeys_path: "/mnt/auth.key",
};

#[derive(Debug)]
pub enum SshError {
    NotImplemented,
    VfsIo,
    NoNetwork,
    NoStorage,
    BadAuthKey,
    Crypto,
}

impl fmt::Display for SshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SshError::NotImplemented => write!(f, "ssh: not implemented yet"),
            SshError::VfsIo          => write!(f, "ssh: vfs i/o"),
            SshError::NoNetwork      => write!(f, "ssh: no network"),
            SshError::NoStorage      => write!(f, "ssh: no /mnt"),
            SshError::BadAuthKey     => write!(f, "ssh: bad authorized_keys"),
            SshError::Crypto         => write!(f, "ssh: crypto failure"),
        }
    }
}

/// One-shot SSH spawn. Currently a stub (Task 1 milestone) — populated as the
/// per-task work lands.
pub fn spawn() -> Result<(), SshError> {
    server::spawn()
}
