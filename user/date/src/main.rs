//! date — read RTC and print current time.
//! Modes:
//!   date              YYYY-MM-DD HH:MM:SS
//!   date +%s          unix epoch
//!   date -u           same as default (RTC assumed UTC)

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn time_get(
        year_ptr: u32, month_ptr: u32, day_ptr: u32,
        hour_ptr: u32, min_ptr: u32, sec_ptr: u32,
        epoch_ptr: u32,
    ) -> i32;
}

fn main() {
    let mut y: u16 = 0;
    let mut m: u8 = 0;
    let mut d: u8 = 0;
    let mut hh: u8 = 0;
    let mut mm: u8 = 0;
    let mut ss: u8 = 0;
    let mut epoch: u64 = 0;
    let r = unsafe {
        time_get(
            &mut y as *mut _ as u32,
            &mut m as *mut _ as u32,
            &mut d as *mut _ as u32,
            &mut hh as *mut _ as u32,
            &mut mm as *mut _ as u32,
            &mut ss as *mut _ as u32,
            &mut epoch as *mut _ as u32,
        )
    };
    if r != 0 { eprintln!("date: time_get errno {}", r); std::process::exit(1); }

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "+%s") {
        println!("{}", epoch);
    } else {
        println!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC", y, m, d, hh, mm, ss);
    }
}
