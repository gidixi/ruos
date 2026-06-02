//! Pure USB topology/descriptor helpers (no hardware access) — unit-tested.

/// Route string for a device on `hub_port` of a hub whose own route is
/// `hub_route` and whose tier is `hub_tier` (root-attached hub = tier 0).
/// xHCI route string = 5 nibbles; nibble `hub_tier` holds this hop's port.
pub fn child_route(hub_route: u32, hub_port: u8, hub_tier: u8) -> u32 {
    hub_route | (((hub_port as u32) & 0xF) << (4 * hub_tier as u32))
}

/// Max tier depth supported (route string is 5 nibbles = 20 bits).
pub const MAX_TIER: u8 = 5;

/// A full/low-speed device reached through a high-speed hub needs a TT.
/// speeds: 1=Full, 2=Low, 3=High, 4=Super (xHCI PSI / hub-status mapping).
pub fn needs_tt(child_speed: u8, hub_speed: u8) -> bool {
    matches!(child_speed, 1 | 2) && hub_speed == 3
}

/// Control-endpoint max packet size by speed.
pub fn max_packet0(speed: u8) -> u16 { match speed { 4 => 512, 3 => 64, _ => 8 } }

/// Decode a USB 2.0 hub port status (wPortStatus, wPortChange) → fields.
pub struct PortStatus { pub connected: bool, pub enabled: bool, pub reset: bool, pub speed: u8 }
pub fn decode_port_status(wstatus: u16, _wchange: u16) -> PortStatus {
    let speed = if wstatus & (1<<9) != 0 { 2 }       // low
        else if wstatus & (1<<10) != 0 { 3 }         // high
        else { 1 };                                   // full
    PortStatus {
        connected: wstatus & 1 != 0,
        enabled:   wstatus & (1<<1) != 0,
        reset:     wstatus & (1<<4) != 0,
        speed,
    }
}

/// Hub descriptor key fields.
pub struct HubDesc { pub nbr_ports: u8, pub tt_think_time: u8, pub pwr_on_2_pwr_good_ms: u16 }
pub fn decode_hub_desc(d: &[u8]) -> Option<HubDesc> {
    if d.len() < 6 { return None; }
    let wch = (d[3] as u16) | ((d[4] as u16) << 8);
    Some(HubDesc {
        nbr_ports: d[2],
        tt_think_time: ((wch >> 5) & 0x3) as u8,
        pwr_on_2_pwr_good_ms: (d[5] as u16) * 2,
    })
}

#[cfg(test)]
mod tests {
    use super::*; extern crate std;
    #[test] fn route() {
        assert_eq!(child_route(0, 3, 0), 3);
        assert_eq!(child_route(3, 2, 1), 3 | (2<<4));
    }
    #[test] fn tt() {
        assert!(needs_tt(1, 3));  assert!(needs_tt(2, 3));
        assert!(!needs_tt(3, 3)); assert!(!needs_tt(1, 4));
    }
    #[test] fn portstatus() {
        let s = decode_port_status(0b0000_0100_0000_0011, 0);
        assert!(s.connected && s.enabled && s.speed == 3);
    }
    #[test] fn hubdesc() {
        let d = [9,0x29,4, 0x00,0x00, 1, 0,0,0];
        let h = decode_hub_desc(&d).unwrap();
        assert_eq!(h.nbr_ports, 4); assert_eq!(h.pwr_on_2_pwr_good_ms, 2);
    }
}
