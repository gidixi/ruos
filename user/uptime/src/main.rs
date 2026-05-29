#[link(wasm_import_module = "ruos")]
extern "C" {
    fn uptime() -> i64;
}

fn main() {
    let centi = unsafe { uptime() } as u64; // timer at 100 Hz = centiseconds
    let total_s = centi / 100;
    let cs = centi % 100;
    let days = total_s / 86400;
    let h = (total_s % 86400) / 3600;
    let m = (total_s % 3600) / 60;
    let s = total_s % 60;
    if days > 0 {
        println!("up {} day(s), {:02}:{:02}:{:02}.{:02}", days, h, m, s, cs);
    } else {
        println!("up {:02}:{:02}:{:02}.{:02}", h, m, s, cs);
    }
}
