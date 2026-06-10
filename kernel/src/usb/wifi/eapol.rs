//! EAPOL-Key (802.1X) frames for the WPA2-PSK 4-way handshake (SP-WIFI-4).
//!
//! Network byte order (big-endian) per 802.1X. Pure framing — the crypto lives
//! in [`super::wpa2`], the 802.11 data-frame wrapping in [`super::ieee80211`],
//! and the handshake state machine in the parent module. CCMP is HW-offloaded:
//! once the 4-way completes, the PTK/GTK go into the chip's key CAM and the MAC
//! encrypts/decrypts — this module only drives the key exchange.

use alloc::vec::Vec;

pub const EAPOL_VERSION:   u8 = 2;
pub const EAPOL_TYPE_KEY:  u8 = 3;
pub const KEY_DESC_TYPE_RSN: u8 = 2;
/// Key-descriptor version 2 = HMAC-SHA1 MIC + AES key-wrap (WPA2-CCMP).
pub const KEY_DESC_VERSION_2: u16 = 2;

// key_info bit fields (big-endian u16 in the frame).
pub const KEY_INFO_KEY_TYPE: u16 = 1 << 3; // 1 = pairwise
pub const KEY_INFO_INSTALL:  u16 = 1 << 6;
pub const KEY_INFO_ACK:      u16 = 1 << 7;
pub const KEY_INFO_MIC:      u16 = 1 << 8;
pub const KEY_INFO_SECURE:   u16 = 1 << 9;
pub const KEY_INFO_ENCRYPTED: u16 = 1 << 12;

/// Byte offset of the 16-byte MIC field within the full 802.1X frame.
pub const MIC_OFFSET: usize = 4 + 77;
pub const MIC_LEN:    usize = 16;
const KEY_FRAME_MIN:  usize = 4 + 95; // 802.1X header + key descriptor (no key data)

/// The EAPOL-Key fields the supplicant needs.
pub struct KeyFrame {
    pub key_info: u16,
    pub replay_counter: [u8; 8],
    pub nonce: [u8; 32],
    pub mic: [u8; 16],
    pub key_data: Vec<u8>,
    /// The full 802.1X frame as received — the msg-3 MIC must be verified over
    /// these exact bytes (with the MIC field zeroed), not a reconstruction, since
    /// the AP may populate key_iv/key_rsc fields we don't model.
    pub raw: Vec<u8>,
}

/// Parse an 802.1X EAPOL-Key frame (the payload after LLC/SNAP). None if it is
/// not a well-formed EAPOL-Key.
pub fn parse(p: &[u8]) -> Option<KeyFrame> {
    if p.len() < KEY_FRAME_MIN || p[1] != EAPOL_TYPE_KEY {
        return None;
    }
    let b = &p[4..]; // EAPOL-Key body
    if b[0] != KEY_DESC_TYPE_RSN {
        return None;
    }
    let key_info = u16::from_be_bytes([b[1], b[2]]);
    let mut replay_counter = [0u8; 8];
    replay_counter.copy_from_slice(&b[5..13]);
    let mut nonce = [0u8; 32];
    nonce.copy_from_slice(&b[13..45]);
    let mut mic = [0u8; 16];
    mic.copy_from_slice(&b[77..93]);
    let kdl = u16::from_be_bytes([b[93], b[94]]) as usize;
    let total = 4 + 95 + kdl;
    let key_data = if b.len() >= 95 + kdl { b[95..95 + kdl].to_vec() } else { Vec::new() };
    let raw = if p.len() >= total { p[..total].to_vec() } else { p.to_vec() };
    Some(KeyFrame { key_info, replay_counter, nonce, mic, key_data, raw })
}

/// Build an EAPOL-Key frame (802.1X header + descriptor). The MIC field is left
/// zero; when `key_info` has KEY_INFO_MIC the caller computes HMAC-SHA1 over the
/// returned bytes with the KCK and patches the result in at `MIC_OFFSET`.
pub fn build(key_info: u16, key_len: u16, replay_counter: &[u8; 8],
             nonce: &[u8; 32], key_data: &[u8]) -> Vec<u8> {
    let body_len = 95 + key_data.len();
    let mut f = Vec::with_capacity(4 + body_len);
    f.push(EAPOL_VERSION);
    f.push(EAPOL_TYPE_KEY);
    f.extend_from_slice(&(body_len as u16).to_be_bytes());
    f.push(KEY_DESC_TYPE_RSN);
    f.extend_from_slice(&key_info.to_be_bytes());
    f.extend_from_slice(&key_len.to_be_bytes());
    f.extend_from_slice(replay_counter);
    f.extend_from_slice(nonce);
    f.extend_from_slice(&[0u8; 16]); // key IV
    f.extend_from_slice(&[0u8; 8]);  // key RSC
    f.extend_from_slice(&[0u8; 8]);  // key ID (reserved)
    f.extend_from_slice(&[0u8; 16]); // key MIC (zero — patched by caller)
    f.extend_from_slice(&(key_data.len() as u16).to_be_bytes());
    f.extend_from_slice(key_data);
    f
}

/// Find the GTK inside the *decrypted* msg-3 key data. The GTK KDE is
/// `dd <len> 00-0F-AC 01 <keyid byte> <reserved> <GTK…>`. Returns
/// (key index, GTK bytes), or None if absent.
pub fn extract_gtk(key_data: &[u8]) -> Option<(u8, Vec<u8>)> {
    let mut i = 0;
    while i + 2 <= key_data.len() {
        let id = key_data[i];
        if id == 0x00 {
            break; // padding
        }
        let len = key_data[i + 1] as usize;
        if i + 2 + len > key_data.len() {
            break;
        }
        let d = &key_data[i + 2..i + 2 + len];
        // KDE (id 0xdd) with OUI 00-0F-AC, data type 1 = GTK.
        if id == 0xdd && d.len() >= 7 && d[0..3] == [0x00, 0x0f, 0xac] && d[3] == 0x01 {
            let key_id = d[4] & 0x03;
            return Some((key_id, d[6..].to_vec()));
        }
        i += 2 + len;
    }
    None
}
