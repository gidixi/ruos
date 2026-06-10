//! wificonnect -- associate with a WPA2 network via the RTL8188EU USB dongle.
//!
//! Thin front-end over the kernel host fn `ruos::wifi_connect`: lazily brings the
//! chip up, scans for the SSID, then runs open-system auth + WPA2 association.
//! Returns a one-line status `auth=.. assoc=.. aid=N`.
//!
//! Usage:
//!   wificonnect <ssid> [password]
//!
//! NOTE: this currently associates only — the WPA2 4-way handshake + CCMP key
//! install (SP-WIFI-3/4) are not yet wired, so the link does not pass traffic
//! yet. `assoc=ok aid=N` confirms the MLME path works end-to-end.

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn wifi_connect(ssid_ptr: i32, ssid_len: i32, pass_ptr: i32, pass_len: i32,
                    buf_ptr: i32, buf_cap: i32) -> i32;
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: wificonnect <ssid> [password]");
        std::process::exit(2);
    }
    let ssid = args[1].as_bytes();
    let pass = if args.len() >= 3 { args[2].as_bytes() } else { &[] };

    let mut buf = [0u8; 256];
    let n = unsafe {
        wifi_connect(
            ssid.as_ptr() as i32, ssid.len() as i32,
            pass.as_ptr() as i32, pass.len() as i32,
            buf.as_mut_ptr() as i32, buf.len() as i32,
        )
    };
    if n < 0 {
        eprintln!("wificonnect: bad arguments (or buffer too small)");
        std::process::exit(1);
    }
    if n == 0 {
        eprintln!("wificonnect: no RTL8188EU dongle");
        std::process::exit(1);
    }
    print!("{}", String::from_utf8_lossy(&buf[..n as usize]));
}
