//! authorized_keys reader (OpenSSH format).
//!
//! Each line: `ssh-ed25519 <base64-encoded-pubkey-blob> [comment]`
//! The blob is RFC 4253 §6.6:
//!   `[u32 len][b"ssh-ed25519"][u32 len][32-byte raw pubkey]`
//! We extract just the trailing 32 bytes.
//!
//! Inline base64 decoder, no external dep.

use alloc::vec;
use alloc::vec::Vec;

use crate::ssh::SshError;
use crate::vfs::{block_on, OpenFlags};

/// Load and parse the authorized_keys file. Missing file = empty list (no
/// authorized clients yet); the spawn path can still publish the server's
/// host key but every login will be rejected at auth time.
pub fn load(path: &str) -> Result<Vec<[u8; 32]>, SshError> {
    let bytes = match read_file(path) {
        Some(b) => b,
        None    => {
            crate::bwarn!("ssh", "{} missing — no authorized clients", path);
            return Ok(Vec::new());
        }
    };
    let text = core::str::from_utf8(&bytes).map_err(|_| SshError::BadAuthKey)?;
    let mut out = Vec::new();
    for (lineno, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        match parse_line(line) {
            Some(k) => out.push(k),
            None    => crate::bwarn!("ssh", "{}:{}: malformed line", path, lineno + 1),
        }
    }
    crate::binfo!("ssh", "loaded {} authorized key(s) from {}", out.len(), path);
    Ok(out)
}

fn read_file(path: &str) -> Option<Vec<u8>> {
    let fd = block_on(crate::vfs::open(path, OpenFlags::READ)).ok()?;
    let mut out = Vec::new();
    let mut chunk = vec![0u8; 1024];
    loop {
        match block_on(crate::vfs::read(fd, &mut chunk)) {
            Ok(0)  => break,
            Ok(n)  => out.extend_from_slice(&chunk[..n]),
            Err(_) => { let _ = block_on(crate::vfs::close(fd)); return None; }
        }
    }
    let _ = block_on(crate::vfs::close(fd));
    Some(out)
}

fn parse_line(line: &str) -> Option<[u8; 32]> {
    let mut it = line.split_whitespace();
    let algo = it.next()?;
    if algo != "ssh-ed25519" { return None; }
    let b64 = it.next()?;
    let blob = base64_decode(b64)?;
    parse_blob(&blob)
}

/// RFC 4253 §6.6 framing: `[u32 BE strlen]b"ssh-ed25519"[u32 BE strlen][32]`.
fn parse_blob(b: &[u8]) -> Option<[u8; 32]> {
    if b.len() < 4 { return None; }
    let algo_len = u32::from_be_bytes(b[0..4].try_into().ok()?) as usize;
    if 4 + algo_len + 4 > b.len() { return None; }
    if &b[4..4 + algo_len] != b"ssh-ed25519" { return None; }
    let kp_off = 4 + algo_len;
    let key_len = u32::from_be_bytes(b[kp_off..kp_off + 4].try_into().ok()?) as usize;
    if key_len != 32 { return None; }
    let key_start = kp_off + 4;
    if key_start + 32 > b.len() { return None; }
    let mut out = [0u8; 32];
    out.copy_from_slice(&b[key_start..key_start + 32]);
    Some(out)
}

/// Minimal RFC 4648 base64 decoder. Returns None on any invalid byte.
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+'        => Some(62),
            b'/'        => Some(63),
            _           => None,
        }
    }
    let mut bytes = Vec::new();
    let s = s.as_bytes();
    let mut i = 0;
    while i < s.len() {
        let mut group = [0u8; 4];
        let mut got = 0usize;
        while got < 4 && i < s.len() {
            let c = s[i]; i += 1;
            if c == b'=' || c.is_ascii_whitespace() { continue; }
            group[got] = val(c)?;
            got += 1;
        }
        if got == 0 { break; }
        if got < 2 { return None; }
        bytes.push((group[0] << 2) | (group[1] >> 4));
        if got >= 3 { bytes.push((group[1] << 4) | (group[2] >> 2)); }
        if got >= 4 { bytes.push((group[2] << 6) |  group[3]      ); }
    }
    Some(bytes)
}
