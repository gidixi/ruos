//! ping — IPv4 ICMP echo. IP literals only (no DNS).
//!
//! Usage:
//!   ping <ip>             4 echoes, ~1 s apart, 1 s timeout
//!   ping -c N <ip>        N echoes
//!   ping -W ms <ip>       per-echo timeout (default 1000 ms)

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn ping(
        ip0: i32, ip1: i32, ip2: i32, ip3: i32,
        timeout_ms: i32,
        latency_ms_ptr: u32,
    ) -> i32;

    fn net_resolve(
        name_ptr: i32,
        name_len: i32,
        addrs_out_ptr: i32,
        max_addrs: i32,
        count_out_ptr: i32,
    ) -> i32;
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
    let mut count: i32 = 4;
    let mut timeout: i32 = 1000;
    let mut target: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-c" => { count   = args.get(i+1).and_then(|s| s.parse().ok()).unwrap_or(count); i += 2; }
            "-W" => { timeout = args.get(i+1).and_then(|s| s.parse().ok()).unwrap_or(timeout); i += 2; }
            other => { target = Some(other.to_string()); i += 1; }
        }
    }
    let ip = match target.clone().and_then(|t| parse_ip4(&t)) {
        Some(v) => v,
        None => {
            let t = match target {
                Some(t) => t,
                None => { eprintln!("usage: ping [-c N] [-W ms] <ip_or_host>"); std::process::exit(2); }
            };
            let mut addrs = [0u8; 4];
            let mut count = 0u32;
            let r = unsafe {
                net_resolve(
                    t.as_ptr() as i32,
                    t.len() as i32,
                    addrs.as_mut_ptr() as i32,
                    1,
                    &mut count as *mut u32 as i32,
                )
            };
            if r != 0 || count == 0 {
                eprintln!("ping: bad address '{}'", t);
                std::process::exit(2);
            }
            [addrs[0], addrs[1], addrs[2], addrs[3]]
        }
    };
    println!("PING {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);

    let mut ok = 0u32;
    let mut lost = 0u32;
    let mut total_ms: u64 = 0;
    for n in 1..=count {
        let mut lat: u32 = 0;
        let r = unsafe {
            ping(ip[0] as i32, ip[1] as i32, ip[2] as i32, ip[3] as i32,
                 timeout, &mut lat as *mut u32 as u32)
        };
        if r == 0 {
            println!("{}.{}.{}.{}: seq={} time={} ms",
                ip[0], ip[1], ip[2], ip[3], n, lat);
            ok += 1;
            total_ms += lat as u64;
        } else {
            println!("{}.{}.{}.{}: seq={} timeout",
                ip[0], ip[1], ip[2], ip[3], n);
            lost += 1;
        }
        // Sleep ~1 s before the next probe (skip after the last one).
        if n < count {
            std::thread::sleep(std::time::Duration::from_millis(1000));
        }
    }
    let total = ok + lost;
    let avg = if ok > 0 { total_ms / ok as u64 } else { 0 };
    println!("--- {}.{}.{}.{} ping statistics ---",
        ip[0], ip[1], ip[2], ip[3]);
    println!("{} packets transmitted, {} received, {}% loss, avg {} ms",
        total, ok, if total > 0 { lost * 100 / total } else { 0 }, avg);
    if ok == 0 { std::process::exit(1); }
}
