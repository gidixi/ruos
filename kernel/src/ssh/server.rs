//! SSH accept loop + per-session task.
//!
//! Task 1: stub only — `spawn()` logs the intent and returns NotImplemented
//! until Task 5 wires the actual socket bind + listen + serve loop.

use crate::ssh::{CONFIG, SshError};

pub fn spawn() -> Result<(), SshError> {
    crate::bwarn!(
        "ssh",
        "spawn skeleton (port {} host_key={} authkeys={}) — pending Tasks 2-8",
        CONFIG.port, CONFIG.host_key_path, CONFIG.authkeys_path,
    );
    Err(SshError::NotImplemented)
}
