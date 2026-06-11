//! Minimal IEEE 802.11 management-frame helpers for the RTL8188EU scan (SP3).
//!
//! This is the *protocol* layer — standard 802.11 frame construction/parsing,
//! independent of the chip. It is correct against the spec regardless of the
//! (still-unverified) RTL register transport: a probe-request builder and a
//! beacon/probe-response information-element parser that yields the SSID,
//! channel and security of a nearby AP.
//!
//! The chip-specific TX/RX descriptors that wrap these frames on the USB bulk
//! pipe, plus channel selection and RF init, belong to SP3b/SP4.

use alloc::string::String;
use alloc::vec::Vec;

/// Security suite inferred from a beacon's capability + information elements.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Security {
    Open,
    Wep,
    Wpa,
    Wpa2,
}

impl Security {
    pub fn as_str(&self) -> &'static str {
        match self {
            Security::Open => "open",
            Security::Wep  => "wep",
            Security::Wpa  => "wpa",
            Security::Wpa2 => "wpa2",
        }
    }
}

/// One access point discovered during a scan.
pub struct ScanResult {
    pub ssid:     String,
    pub bssid:    [u8; 6],
    pub channel:  u8,
    pub security: Security,
}

// 802.11 frame-control first byte (protocol ver 0): type<<2 | subtype<<4.
const FC_ASSOC_REQ:   u8 = 0x00; // mgmt(0), subtype assoc-req(0)
const FC_ASSOC_RESP:  u8 = 0x10; // mgmt(0), subtype assoc-resp(1)
const FC_PROBE_REQ:   u8 = 0x40; // mgmt(0), subtype probe-req(4)
const FC_BEACON:      u8 = 0x80; // mgmt(0), subtype beacon(8)
const FC_PROBE_RESP:  u8 = 0x50; // mgmt(0), subtype probe-resp(5)
const FC_AUTH:        u8 = 0xB0; // mgmt(0), subtype auth(11)

// Information-element ids.
const IE_SSID:        u8 = 0;
const IE_SUPP_RATES:  u8 = 1;
const IE_DS_PARAM:    u8 = 3;  // current channel
const IE_RSN:         u8 = 48; // RSN (WPA2/RSNA)
const IE_VENDOR:      u8 = 221;

// Capability info "Privacy" bit (WEP when set without RSN/WPA IEs).
const CAP_PRIVACY: u16 = 1 << 4;

/// Build a probe-request management frame for `ssid` (empty slice = wildcard,
/// i.e. broadcast probe). Layout: 24-byte MAC header + SSID IE + supported-rates
/// IE. `sa` is our station MAC. The chip TX descriptor is prepended elsewhere.
pub fn build_probe_request(sa: [u8; 6], da: [u8; 6], ssid: &[u8], seq: u16) -> Vec<u8> {
    let mut f = Vec::with_capacity(28 + ssid.len());
    // Frame Control (2) + Duration (2).
    f.extend_from_slice(&[FC_PROBE_REQ, 0x00, 0x00, 0x00]);
    f.extend_from_slice(&da);        // Addr1 = DA (broadcast, or a specific BSSID)
    f.extend_from_slice(&sa);        // Addr2 = SA (our MAC)
    f.extend_from_slice(&da);        // Addr3 = BSSID = DA
    // Sequence control: fragment(0) | sequence-number << 4.
    f.extend_from_slice(&((seq & 0x0FFF) << 4).to_le_bytes());
    // SSID IE.
    f.push(IE_SSID);
    f.push(ssid.len() as u8);
    f.extend_from_slice(ssid);
    // Supported-rates IE: 1,2,5.5,11,6,9,12,18 Mbps (b/g set; high bit = basic).
    f.push(IE_SUPP_RATES);
    f.push(8);
    f.extend_from_slice(&[0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]);
    f
}

/// Parse a received beacon / probe-response (frame starting at the MAC header).
/// Returns the AP descriptor, or None if this isn't a beacon/probe-response or
/// the frame is malformed.
pub fn parse_beacon(frame: &[u8]) -> Option<ScanResult> {
    // 24B MAC header + 12B fixed (timestamp 8 + beacon interval 2 + caps 2) = 36.
    if frame.len() < 36 {
        return None;
    }
    let fc0 = frame[0];
    if fc0 != FC_BEACON && fc0 != FC_PROBE_RESP {
        return None;
    }
    let mut bssid = [0u8; 6];
    bssid.copy_from_slice(&frame[16..22]); // Addr3
    let caps = (frame[34] as u16) | ((frame[35] as u16) << 8);

    let mut ssid = String::new();
    let mut channel = 0u8;
    let mut has_rsn = false;
    let mut has_wpa = false;

    // Walk the tagged information elements starting at offset 36.
    let mut pos = 36;
    while pos + 2 <= frame.len() {
        let id = frame[pos];
        let len = frame[pos + 1] as usize;
        if pos + 2 + len > frame.len() {
            break;
        }
        let data = &frame[pos + 2..pos + 2 + len];
        match id {
            IE_SSID => ssid = String::from_utf8_lossy(data).into_owned(),
            IE_DS_PARAM if len >= 1 => channel = data[0],
            IE_RSN => has_rsn = true,
            // Microsoft WPA vendor IE: OUI 00:50:f2, type 01.
            IE_VENDOR if len >= 4 && data[0..3] == [0x00, 0x50, 0xf2] && data[3] == 0x01 => {
                has_wpa = true;
            }
            _ => {}
        }
        pos += 2 + len;
    }

    let security = if has_rsn {
        Security::Wpa2
    } else if has_wpa {
        Security::Wpa
    } else if caps & CAP_PRIVACY != 0 {
        Security::Wep
    } else {
        Security::Open
    };

    Some(ScanResult { ssid, bssid, channel, security })
}

// ── MLME: authentication + association (SP-WIFI-2) ───────────────────────────
// Standard 802.11 management frames, chip-independent. In Linux these live in
// mac80211; ruos builds them itself. The 24-byte MAC header for a frame TO the
// AP: Addr1 = BSSID (the AP), Addr2 = SA (us), Addr3 = BSSID.

/// Capability info advertised in the assoc-request: ESS + Privacy (WPA2).
const CAP_ESS: u16 = 1 << 0;

/// Push the common 24-byte management header for a frame addressed to `bssid`.
fn push_mgmt_header(f: &mut Vec<u8>, fc0: u8, sa: [u8; 6], bssid: [u8; 6], seq: u16) {
    f.extend_from_slice(&[fc0, 0x00, 0x00, 0x00]); // FC + Duration
    f.extend_from_slice(&bssid);                   // Addr1 = DA = BSSID
    f.extend_from_slice(&sa);                       // Addr2 = SA
    f.extend_from_slice(&bssid);                    // Addr3 = BSSID
    f.extend_from_slice(&((seq & 0x0FFF) << 4).to_le_bytes()); // seq ctrl
}

/// Build an open-system authentication request (algorithm 0, transaction 1).
pub fn build_auth_request(sa: [u8; 6], bssid: [u8; 6], seq: u16) -> Vec<u8> {
    let mut f = Vec::with_capacity(30);
    push_mgmt_header(&mut f, FC_AUTH, sa, bssid, seq);
    f.extend_from_slice(&0u16.to_le_bytes());  // auth algorithm = Open System
    f.extend_from_slice(&1u16.to_le_bytes());  // transaction sequence = 1
    f.extend_from_slice(&0u16.to_le_bytes());  // status code = 0
    f
}

/// The WPA2-PSK / CCMP RSN information element (suite OUI 00-0F-AC):
/// group + pairwise = CCMP(4), AKM = PSK(2).
fn push_rsn_ie_wpa2_psk(f: &mut Vec<u8>) {
    f.push(IE_RSN);
    f.push(20);                                  // length
    f.extend_from_slice(&1u16.to_le_bytes());    // RSN version 1
    f.extend_from_slice(&[0x00, 0x0f, 0xac, 0x04]); // group cipher = CCMP
    f.extend_from_slice(&1u16.to_le_bytes());    // pairwise count
    f.extend_from_slice(&[0x00, 0x0f, 0xac, 0x04]); // pairwise = CCMP
    f.extend_from_slice(&1u16.to_le_bytes());    // AKM count
    f.extend_from_slice(&[0x00, 0x0f, 0xac, 0x02]); // AKM = PSK
    f.extend_from_slice(&0u16.to_le_bytes());    // RSN capabilities
}

/// Build an association request carrying the WPA2-PSK RSN IE.
pub fn build_assoc_request(sa: [u8; 6], bssid: [u8; 6], ssid: &[u8], seq: u16) -> Vec<u8> {
    let mut f = Vec::with_capacity(28 + ssid.len() + 32);
    push_mgmt_header(&mut f, FC_ASSOC_REQ, sa, bssid, seq);
    f.extend_from_slice(&(CAP_ESS | CAP_PRIVACY).to_le_bytes()); // capability info
    f.extend_from_slice(&10u16.to_le_bytes());                   // listen interval
    // SSID IE.
    f.push(IE_SSID);
    f.push(ssid.len() as u8);
    f.extend_from_slice(ssid);
    // Supported-rates IE (b/g set).
    f.push(IE_SUPP_RATES);
    f.push(8);
    f.extend_from_slice(&[0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]);
    // RSN IE (WPA2-PSK / CCMP) — required or a WPA2 AP rejects the association.
    push_rsn_ie_wpa2_psk(&mut f);
    f
}

/// Parse an authentication response. Returns the status code (0 = success) if
/// this is an open-system auth frame with transaction sequence 2, else None.
pub fn parse_auth_response(frame: &[u8]) -> Option<u16> {
    if frame.len() < 30 || frame[0] != FC_AUTH {
        return None;
    }
    let algo = u16::from_le_bytes([frame[24], frame[25]]);
    let txn  = u16::from_le_bytes([frame[26], frame[27]]);
    let status = u16::from_le_bytes([frame[28], frame[29]]);
    if algo != 0 || txn != 2 {
        return None;
    }
    Some(status)
}

/// Parse an association response. Returns (status code, AID) — AID valid only
/// when status == 0. None if this isn't an assoc-response.
pub fn parse_assoc_response(frame: &[u8]) -> Option<(u16, u16)> {
    if frame.len() < 30 || frame[0] != FC_ASSOC_RESP {
        return None;
    }
    let status = u16::from_le_bytes([frame[26], frame[27]]);
    let aid = u16::from_le_bytes([frame[28], frame[29]]) & 0x3FFF;
    Some((status, aid))
}

/// The WPA2-PSK / CCMP RSN information element as a standalone byte vector — the
/// assoc-request and the 4-way msg-2 must carry the *same* IE (the AP checks it).
pub fn rsn_ie_wpa2_psk() -> Vec<u8> {
    let mut f = Vec::with_capacity(22);
    push_rsn_ie_wpa2_psk(&mut f);
    f
}

// ── 802.11 data frames carrying EAPOL (SP-WIFI-4) ────────────────────────────
const FC_DATA:    u8 = 0x08;        // type data(2), subtype data(0)
const FC1_TODS:   u8 = 0x01;        // ToDS: STA -> AP
const LLC_SNAP_EAPOL: [u8; 8] = [0xAA, 0xAA, 0x03, 0x00, 0x00, 0x00, 0x88, 0x8E];

/// Wrap an EAPOL payload in an 802.11 data frame (ToDS, STA→AP) + LLC/SNAP.
/// Addr1 = BSSID (RA), Addr2 = SA (us), Addr3 = DA = AP.
pub fn build_eapol_data(sa: [u8; 6], bssid: [u8; 6], eapol: &[u8], seq: u16) -> Vec<u8> {
    let mut f = Vec::with_capacity(24 + 8 + eapol.len());
    f.extend_from_slice(&[FC_DATA, FC1_TODS, 0x00, 0x00]);
    f.extend_from_slice(&bssid); // Addr1 = RA = BSSID
    f.extend_from_slice(&sa);    // Addr2 = TA = us
    f.extend_from_slice(&bssid); // Addr3 = DA = AP
    f.extend_from_slice(&((seq & 0x0FFF) << 4).to_le_bytes());
    f.extend_from_slice(&LLC_SNAP_EAPOL);
    f.extend_from_slice(eapol);
    f
}

/// Extract the EAPOL payload from a received 802.11 data frame (FromDS, AP→STA).
/// Handles both plain data (24B header) and QoS data (26B). None if not an
/// EAPOL-bearing data frame.
pub fn parse_eapol_data(frame: &[u8]) -> Option<&[u8]> {
    if frame.len() < 24 + 8 || frame[0] & 0x0C != 0x08 {
        return None; // not a data-type frame
    }
    let qos = frame[0] & 0xF0 == 0x80; // subtype 8 = QoS data → 2 extra header bytes
    let hdr = if qos { 26 } else { 24 };
    if frame.len() < hdr + 8 || frame[hdr..hdr + 8] != LLC_SNAP_EAPOL {
        return None;
    }
    Some(&frame[hdr + 8..])
}

// ── Encrypted datapath: 802.3 ↔ 802.11 data frames (SP-WIFI-5) ───────────────
const FC1_PROTECTED:  u8 = 0x40; // FC byte1: frame body is/was CCMP-protected
const CCMP_HDR_LEN:   usize = 8;
const CCMP_MIC_LEN:   usize = 8;
/// LLC/SNAP prefix (RFC 1042): the EtherType follows these 6 bytes.
const LLC_SNAP_PREFIX: [u8; 6] = [0xAA, 0xAA, 0x03, 0x00, 0x00, 0x00];

/// Build the 8-byte CCMP header for an encrypted TX frame (mac80211 layout:
/// SW writes the header + PN, HW does AES-CCM). `pn` is the 48-bit packet number,
/// `key_id` the pairwise key index (0). The Ext-IV bit (0x20) is always set.
pub fn ccmp_header(pn: u64, key_id: u8) -> [u8; 8] {
    [
        (pn & 0xff) as u8,            // PN0
        ((pn >> 8) & 0xff) as u8,     // PN1
        0x00,                         // reserved
        (key_id << 6) | 0x20,         // KeyID | Ext-IV
        ((pn >> 16) & 0xff) as u8,    // PN2
        ((pn >> 24) & 0xff) as u8,    // PN3
        ((pn >> 32) & 0xff) as u8,    // PN4
        ((pn >> 40) & 0xff) as u8,    // PN5
    ]
}

/// Wrap an Ethernet-II frame (`eth` = dst[6] src[6] ethertype[2] payload…) into
/// an 802.11 data frame to the AP (ToDS, Protected): Addr1=RA=BSSID, Addr2=TA=us,
/// Addr3=DA=the Ethernet dst. Inserts the 8-byte CCMP header + LLC/SNAP; the HW
/// encrypts the body + appends the MIC. STA→AP is always Addr1=BSSID, so even a
/// broadcast Ethernet dst rides the pairwise key (the AP relays).
pub fn build_data_frame(sa: [u8; 6], bssid: [u8; 6], eth: &[u8], seq: u16, ccmp: &[u8; 8]) -> Vec<u8> {
    if eth.len() < 14 {
        return Vec::new();
    }
    let payload = &eth[14..];
    let mut f = Vec::with_capacity(24 + CCMP_HDR_LEN + 8 + payload.len());
    f.extend_from_slice(&[0x08, FC1_TODS | FC1_PROTECTED, 0x00, 0x00]); // FC(data,ToDS,Prot)+Dur
    f.extend_from_slice(&bssid);          // Addr1 = RA = BSSID
    f.extend_from_slice(&sa);             // Addr2 = TA = us
    f.extend_from_slice(&eth[0..6]);      // Addr3 = DA = Ethernet dst
    f.extend_from_slice(&((seq & 0x0FFF) << 4).to_le_bytes());
    f.extend_from_slice(ccmp);            // CCMP header (HW encrypts past here)
    f.extend_from_slice(&LLC_SNAP_PREFIX);
    f.extend_from_slice(&eth[12..14]);    // EtherType
    f.extend_from_slice(payload);
    f
}

/// Decapsulate a HW-decrypted RX 802.11 data frame (FromDS, AP→STA) into an
/// Ethernet-II frame for smoltcp. Strips the MAC header (24/26B), the CCMP
/// header (8B, when Protected) + trailing MIC (8B), and the LLC/SNAP. Returns
/// `dst src ethertype payload`, or None if not a SNAP-bearing data frame.
pub fn parse_data_frame(frame: &[u8]) -> Option<Vec<u8>> {
    if frame.len() < 24 || frame[0] & 0x0C != 0x08 {
        return None; // not a data-type frame
    }
    let qos = frame[0] & 0xF0 == 0x80;          // subtype 8 = QoS data
    let protected = frame[1] & FC1_PROTECTED != 0;
    let hdr = if qos { 26 } else { 24 };
    // FromDS infra: Addr1 = DA (us / bcast), Addr3 = SA.
    let mut da = [0u8; 6];
    let mut sa = [0u8; 6];
    da.copy_from_slice(&frame[4..10]);
    sa.copy_from_slice(&frame[16..22]);
    let mut off = hdr;
    if protected { off += CCMP_HDR_LEN; }
    if frame.len() < off + 8 || frame[off..off + 6] != LLC_SNAP_PREFIX {
        return None;
    }
    let ethertype = [frame[off + 6], frame[off + 7]];
    let body_start = off + 8;
    let body_end = if protected { frame.len().checked_sub(CCMP_MIC_LEN)? } else { frame.len() };
    if body_end <= body_start {
        return None;
    }
    let mut out = Vec::with_capacity(14 + (body_end - body_start));
    out.extend_from_slice(&da);
    out.extend_from_slice(&sa);
    out.extend_from_slice(&ethertype);
    out.extend_from_slice(&frame[body_start..body_end]);
    Some(out)
}
