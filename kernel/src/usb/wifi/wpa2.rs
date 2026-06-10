//! WPA2-PSK 4-way-handshake supplicant crypto (SP-WIFI-3).
//!
//! Pure offline math — independent of the RTL8188EU transport. CCMP itself is
//! HW-offloaded by the chip's key CAM, so this only implements the *control
//! plane*: deriving the keys, authenticating the EAPOL frames, and unwrapping
//! the group key that the AP delivers inside message 3.
//!
//! WPA2-PSK is key-descriptor **version 2** → HMAC-SHA1 / AES (NOT SHA-256):
//!   PMK  = PBKDF2-HMAC-SHA1(passphrase, ssid, 4096, 32)
//!   PTK  = PRF-384(PMK, "Pairwise key expansion",
//!                  min(AA,SPA)‖max(AA,SPA)‖min(ANonce,SNonce)‖max(ANonce,SNonce))
//!          split → KCK[0..16] | KEK[16..32] | TK[32..48]
//!   MIC  = HMAC-SHA1(KCK, eapol-with-MIC-zeroed)[0..16]
//!   GTK  = AES-128 key-unwrap (RFC 3394) of msg-3 key data, KEK as the KEK
//!
//! Verified offline against known-answer vectors in [`selftest`] (boot-checks):
//! the IEEE 802.11i PMK vector exercises the whole PBKDF2/HMAC-SHA1 chain that
//! the PRF and MIC reuse, and the RFC 3394 vector exercises the AES unwrap.

use alloc::vec::Vec;
use hmac::{Hmac, Mac};
use sha1::Sha1;

type HmacSha1 = Hmac<Sha1>;

/// Pairwise Transient Key split for CCMP (version-2 / 48-byte PTK).
#[derive(Clone, Copy)]
pub struct Ptk {
    /// EAPOL-Key Confirmation Key — keys the message MIC.
    pub kck: [u8; 16],
    /// EAPOL-Key Encryption Key — unwraps the GTK in message 3.
    pub kek: [u8; 16],
    /// Temporal Key — the per-association CCMP key installed in the chip CAM.
    pub tk: [u8; 16],
}

/// PMK = PBKDF2-HMAC-SHA1(passphrase, ssid, 4096 iterations, 32 bytes).
pub fn pmk(passphrase: &[u8], ssid: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha1>(passphrase, ssid, 4096, &mut out);
    out
}

/// IEEE 802.11 PRF over HMAC-SHA1 (wpa_supplicant `sha1_prf`): each block hashes
/// `label ‖ 0x00 ‖ data ‖ counter`, the counter a single byte from 0. The null
/// byte is the label's C-string terminator (`strlen(label)+1` in the reference).
fn sha1_prf(key: &[u8], label: &[u8], data: &[u8], out: &mut [u8]) {
    let mut counter: u8 = 0;
    let mut pos = 0;
    while pos < out.len() {
        let mut mac = HmacSha1::new_from_slice(key).expect("hmac accepts any key length");
        mac.update(label);
        mac.update(&[0u8]);
        mac.update(data);
        mac.update(&[counter]);
        let digest = mac.finalize().into_bytes(); // 20 bytes
        let n = core::cmp::min(digest.len(), out.len() - pos);
        out[pos..pos + n].copy_from_slice(&digest[..n]);
        pos += n;
        counter += 1;
    }
}

/// Derive the PTK (CCMP, 384-bit) from the PMK, the two MACs and the two nonces.
/// `aa` = authenticator (AP) MAC, `spa` = supplicant (our) MAC.
pub fn derive_ptk(pmk: &[u8; 32], aa: &[u8; 6], spa: &[u8; 6],
                  anonce: &[u8; 32], snonce: &[u8; 32]) -> Ptk {
    // data = min‖max of the MACs, then min‖max of the nonces (byte-lexicographic).
    let mut data = [0u8; 6 + 6 + 32 + 32];
    let (lo_mac, hi_mac) = if aa.as_slice() < spa.as_slice() { (aa, spa) } else { (spa, aa) };
    let (lo_n, hi_n) = if anonce.as_slice() < snonce.as_slice() { (anonce, snonce) } else { (snonce, anonce) };
    data[0..6].copy_from_slice(lo_mac);
    data[6..12].copy_from_slice(hi_mac);
    data[12..44].copy_from_slice(lo_n);
    data[44..76].copy_from_slice(hi_n);

    let mut ptk = [0u8; 48];
    sha1_prf(pmk, b"Pairwise key expansion", &data, &mut ptk);

    let mut k = Ptk { kck: [0; 16], kek: [0; 16], tk: [0; 16] };
    k.kck.copy_from_slice(&ptk[0..16]);
    k.kek.copy_from_slice(&ptk[16..32]);
    k.tk.copy_from_slice(&ptk[32..48]);
    k
}

/// EAPOL-Key MIC (version 2): HMAC-SHA1 over the whole EAPOL frame with the MIC
/// field zeroed, truncated to 16 bytes. Caller zeroes the MIC field first.
pub fn eapol_mic(kck: &[u8; 16], eapol: &[u8]) -> [u8; 16] {
    let mut mac = HmacSha1::new_from_slice(kck).expect("hmac accepts any key length");
    mac.update(eapol);
    let digest = mac.finalize().into_bytes();
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest[..16]);
    out
}

/// AES-128 key unwrap (RFC 3394) of `wrapped` (n+1 64-bit blocks) with `kek`.
/// Returns the unwrapped key data (n blocks), or None if the integrity check
/// fails (wrong KEK / corrupt data) or the length is malformed.
pub fn aes_unwrap(kek: &[u8; 16], wrapped: &[u8]) -> Option<Vec<u8>> {
    use aes::Aes128;
    use aes::cipher::{BlockDecrypt, KeyInit};
    use aes::cipher::generic_array::GenericArray;

    if wrapped.len() < 16 || wrapped.len() % 8 != 0 { return None; }
    let n = wrapped.len() / 8 - 1;
    let cipher = Aes128::new(GenericArray::from_slice(kek));

    let mut a = [0u8; 8];
    a.copy_from_slice(&wrapped[0..8]);
    let mut r: Vec<[u8; 8]> = Vec::with_capacity(n);
    for i in 0..n {
        let mut blk = [0u8; 8];
        blk.copy_from_slice(&wrapped[8 * (i + 1)..8 * (i + 2)]);
        r.push(blk);
    }

    for j in (0..6u64).rev() {
        for i in (1..=n).rev() {
            let t = (n as u64) * j + i as u64;
            let mut block = [0u8; 16];
            let tb = t.to_be_bytes();
            let mut ax = a;
            for k in 0..8 { ax[k] ^= tb[k]; }
            block[0..8].copy_from_slice(&ax);
            block[8..16].copy_from_slice(&r[i - 1]);
            let mut ga = GenericArray::clone_from_slice(&block);
            cipher.decrypt_block(&mut ga);
            a.copy_from_slice(&ga[0..8]);
            r[i - 1].copy_from_slice(&ga[8..16]);
        }
    }

    if a != [0xA6u8; 8] { return None; }
    let mut out = Vec::with_capacity(n * 8);
    for blk in &r { out.extend_from_slice(blk); }
    Some(out)
}

/// Offline known-answer self-test (boot-checks). Proves the PBKDF2/HMAC-SHA1
/// chain (PMK vector, IEEE 802.11i) and the AES-128 unwrap (RFC 3394 §4.1).
#[cfg(feature = "boot-checks")]
pub fn selftest() -> Result<(), &'static str> {
    // IEEE 802.11i PMK: ssid "IEEE", passphrase "password".
    let want_pmk: [u8; 32] = [
        0xf4, 0x2c, 0x6f, 0xc5, 0x2d, 0xf0, 0xeb, 0xef, 0x9e, 0xbb, 0x4b, 0x90, 0xb3, 0x8a, 0x5f, 0x90,
        0x2e, 0x83, 0xfe, 0x1b, 0x13, 0x5a, 0x70, 0xe2, 0x3a, 0xed, 0x76, 0x2e, 0x97, 0x10, 0xa1, 0x2e,
    ];
    if pmk(b"password", b"IEEE") != want_pmk { return Err("pmk vector mismatch"); }

    // RFC 3394 §4.1: 128-bit KEK, 128-bit key.
    let kek: [u8; 16] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
    ];
    let wrapped: [u8; 24] = [
        0x1f, 0xa6, 0x8b, 0x0a, 0x81, 0x12, 0xb4, 0x47, 0xae, 0xf3, 0x4b, 0xd8, 0xfb, 0x5a, 0x7b, 0x82,
        0x9d, 0x3e, 0x86, 0x23, 0x71, 0xd2, 0xcf, 0xe5,
    ];
    let want_plain: [u8; 16] = [
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
    ];
    match aes_unwrap(&kek, &wrapped) {
        Some(p) if p.as_slice() == want_plain => {}
        Some(_) => return Err("aes-unwrap result mismatch"),
        None    => return Err("aes-unwrap integrity check failed"),
    }
    Ok(())
}
