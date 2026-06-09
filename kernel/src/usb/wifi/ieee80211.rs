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
const FC_PROBE_REQ:   u8 = 0x40; // mgmt(0), subtype probe-req(4)
const FC_BEACON:      u8 = 0x80; // mgmt(0), subtype beacon(8)
const FC_PROBE_RESP:  u8 = 0x50; // mgmt(0), subtype probe-resp(5)

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
