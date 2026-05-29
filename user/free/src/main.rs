#[link(wasm_import_module = "ruos")]
extern "C" {
    fn meminfo(buf_ptr: u32) -> i32;
}

fn human(b: u64) -> String {
    let units = ["B", "K", "M", "G"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 { v /= 1024.0; i += 1; }
    if i == 0 { format!("{}{}", b, units[0]) } else { format!("{:.1}{}", v, units[i]) }
}

fn main() {
    let mut buf = [0u8; 32];
    let errno = unsafe { meminfo(buf.as_mut_ptr() as u32) };
    if errno != 0 {
        eprintln!("free: errno {}", errno);
        std::process::exit(1);
    }
    let heap_total = u64::from_le_bytes(buf[0..8].try_into().unwrap());
    let heap_used  = u64::from_le_bytes(buf[8..16].try_into().unwrap());
    let f_total    = u64::from_le_bytes(buf[16..24].try_into().unwrap()) * 4096;
    let f_used     = u64::from_le_bytes(buf[24..32].try_into().unwrap()) * 4096;
    let args: Vec<String> = std::env::args().collect();
    let h = args.iter().any(|a| a == "-h");
    let fmt = |b: u64| -> String { if h { human(b) } else { format!("{}", b) } };
    println!("{:12} {:>12} {:>12} {:>12}", "", "total", "used", "free");
    println!("{:12} {:>12} {:>12} {:>12}",
        "phys frames:",
        fmt(f_total),
        fmt(f_used),
        fmt(f_total.saturating_sub(f_used)));
    println!("{:12} {:>12} {:>12} {:>12}",
        "kernel heap:",
        fmt(heap_total),
        if heap_used == 0 { "?".to_string() } else { fmt(heap_used) },
        if heap_used == 0 { "?".to_string() } else { fmt(heap_total - heap_used) });
}
