#[link(wasm_import_module = "ruos")]
extern "C" {
    fn proc_list(buf_ptr: u32, buf_len: u32, used_ptr: u32) -> i32;
    fn uptime() -> i64;
}

fn main() {
    let mut buf = vec![0u8; 8192];
    let mut used: u32 = 0;
    let errno = unsafe {
        proc_list(buf.as_mut_ptr() as u32, buf.len() as u32, &mut used as *mut u32 as u32)
    };
    if errno != 0 {
        eprintln!("ps: errno {}", errno);
        std::process::exit(1);
    }
    let used = used as usize;
    if used < 4 {
        println!("PID  ELAPSED  CMD");
        return;
    }
    let count = u32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
    let now = unsafe { uptime() } as u64;
    println!("{:>5} {:>10} {}", "PID", "ELAPSED", "CMD");
    let mut o = 4usize;
    for _ in 0..count {
        if o + 14 > used { break; }
        let pid = u32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
        let start = u64::from_le_bytes(buf[o + 4..o + 12].try_into().unwrap());
        let nl = u16::from_le_bytes(buf[o + 12..o + 14].try_into().unwrap()) as usize;
        // skip pad (2 bytes) then name
        let name_start = o + 16;
        if name_start + nl > used { break; }
        let name = std::str::from_utf8(&buf[name_start..name_start + nl]).unwrap_or("?");
        let elapsed_cs = now.saturating_sub(start);
        let s = elapsed_cs / 100;
        let cs = elapsed_cs % 100;
        println!("{:>5} {:>7}.{:02} {}", pid, s, cs, name);
        o = name_start + nl;
    }
}
