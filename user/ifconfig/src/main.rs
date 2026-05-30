//! ifconfig — show interfaces or set static IP / restart DHCP.
//!
//! Usage:
//!   ifconfig                                   show all
//!   ifconfig eth0 <ip>/<prefix> [gw <gw>]      set static
//!   ifconfig eth0 dhcp                          restart DHCP

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn net_iface(buf_ptr: u32, buf_len: u32, used_ptr: u32) -> i32;
    fn net_set_static(
        ip0: i32, ip1: i32, ip2: i32, ip3: i32,
        prefix: i32,
        gw0: i32, gw1: i32, gw2: i32, gw3: i32,
        gw_present: i32,
    ) -> i32;
    fn net_dhcp_renew() -> i32;
}

fn show_all() {
    let mut buf = vec![0u8; 2048];
    let mut used: u32 = 0;
    let errno = unsafe {
        net_iface(buf.as_mut_ptr() as u32, buf.len() as u32, &mut used as *mut u32 as u32)
    };
    if errno == 8 {
        buf = vec![0u8; used as usize];
        let _ = unsafe {
            net_iface(buf.as_mut_ptr() as u32, buf.len() as u32, &mut used as *mut u32 as u32)
        };
    } else if errno != 0 {
        eprintln!("ifconfig: net_iface errno {}", errno);
        std::process::exit(1);
    }
    print!("{}", String::from_utf8_lossy(&buf[..used as usize]));
}

fn parse_ip4(s: &str) -> Option<[u8; 4]> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 { return None; }
    let mut out = [0u8; 4];
    for (i, p) in parts.iter().enumerate() { out[i] = p.parse().ok()?; }
    Some(out)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() { show_all(); return; }
    // We don't actually multiplex per-interface yet; iface name is informational
    // (currently a single Ethernet iface is active).
    let _iface = &args[0];
    if args.get(1).map(|s| s.as_str()) == Some("dhcp") {
        let r = unsafe { net_dhcp_renew() };
        if r != 0 { eprintln!("ifconfig: dhcp errno {}", r); std::process::exit(1); }
        println!("ifconfig: DHCP restart requested");
        return;
    }
    // Static: ip/prefix [gw <gw>]
    let cidr = match args.get(1) {
        Some(s) => s.clone(),
        None    => { eprintln!("ifconfig: missing <ip>/<prefix>"); std::process::exit(2); }
    };
    let (ip_s, prefix_s) = match cidr.split_once('/') {
        Some(t) => t,
        None    => { eprintln!("ifconfig: ip must be in CIDR form (e.g. 192.168.1.10/24)"); std::process::exit(2); }
    };
    let ip = match parse_ip4(ip_s) {
        Some(v) => v,
        None    => { eprintln!("ifconfig: bad IP {}", ip_s); std::process::exit(2); }
    };
    let prefix: i32 = match prefix_s.parse() {
        Ok(p) => p,
        Err(_) => { eprintln!("ifconfig: bad prefix {}", prefix_s); std::process::exit(2); }
    };
    let mut gw = [0u8; 4];
    let mut gw_present = 0;
    if let Some(pos) = args.iter().position(|a| a == "gw") {
        if let Some(g) = args.get(pos + 1).and_then(|s| parse_ip4(s)) {
            gw = g;
            gw_present = 1;
        } else {
            eprintln!("ifconfig: bad gw");
            std::process::exit(2);
        }
    }
    let r = unsafe {
        net_set_static(
            ip[0] as i32, ip[1] as i32, ip[2] as i32, ip[3] as i32,
            prefix,
            gw[0] as i32, gw[1] as i32, gw[2] as i32, gw[3] as i32,
            gw_present,
        )
    };
    if r != 0 { eprintln!("ifconfig: errno {}", r); std::process::exit(1); }
    println!("ifconfig: applied static IP");
}
