//! df: ruos has only tmpfs / mounted at /, sized by phys frame budget.

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
        eprintln!("df: errno {}", errno);
        std::process::exit(1);
    }
    let f_total = u64::from_le_bytes(buf[16..24].try_into().unwrap()) * 4096;
    let f_used  = u64::from_le_bytes(buf[24..32].try_into().unwrap()) * 4096;
    let avail   = f_total.saturating_sub(f_used);
    let pct     = if f_total > 0 { (f_used * 100) / f_total } else { 0 };
    let args: Vec<String> = std::env::args().collect();
    let h = args.iter().any(|a| a == "-h");
    let fmt = |b: u64| if h { human(b) } else { format!("{}", b / 1024) };
    println!("{:<12} {:>10} {:>10} {:>10} {:>5} {}",
        "Filesystem", "Size", "Used", "Avail", "Use%", "Mounted");
    println!("{:<12} {:>10} {:>10} {:>10} {:>4}% {}",
        "tmpfs", fmt(f_total), fmt(f_used), fmt(avail), pct, "/");
}
