//! wifiscan -- scan for nearby WiFi networks via the RTL8188EU USB dongle.
//!
//! Thin front-end over the kernel host fn `ruos::wifi_scan`, which lazily brings
//! the chip up (power-on + firmware + MAC/BB/RF) on the first call, runs a passive
//! 2.4 GHz scan, and returns one AP per line: `<ssid>\t<channel>\t<security>`.
//!
//! Usage:
//!   wifiscan    list nearby networks (first run takes ~1-2 s for chip bring-up)

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn wifi_scan(ptr: i32, cap: i32) -> i32;
}

fn main() {
    let mut buf = [0u8; 4096];
    let n = unsafe { wifi_scan(buf.as_mut_ptr() as i32, buf.len() as i32) };
    if n < 0 {
        eprintln!("wifiscan: result too large (or no WiFi device)");
        std::process::exit(1);
    }
    if n == 0 {
        println!("wifiscan: no networks found (no RTL8188EU dongle, or none in range)");
        return;
    }
    println!("SSID                              CH   SECURITY");
    let text = String::from_utf8_lossy(&buf[..n as usize]);
    for line in text.lines() {
        let mut f = line.split('\t'); // ssid \t channel \t security
        let ssid = f.next().unwrap_or("?");
        let ch = f.next().unwrap_or("?");
        let sec = f.next().unwrap_or("?");
        println!("{:<32}  {:>2}   {}", ssid, ch, sec);
    }
}
