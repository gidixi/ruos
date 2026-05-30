//! SSH accept loop + per-session task.
//!
//! Task 2 milestone: load/generate host key. Real socket bind + transport
//! lands in Tasks 4-5.

use crate::ssh::{hostkey, CONFIG, SshError};

pub fn spawn() -> Result<(), SshError> {
    let key = hostkey::load_or_generate(CONFIG.host_key_path)?;
    let pub_bytes = key.public();
    crate::binfo!(
        "ssh",
        "host key fingerprint {:02x}{:02x}{:02x}{:02x}…{:02x}{:02x}",
        pub_bytes[0], pub_bytes[1], pub_bytes[2], pub_bytes[3],
        pub_bytes[30], pub_bytes[31],
    );
    crate::bwarn!("ssh", "transport pending Tasks 4-5");
    Err(SshError::NotImplemented)
}
