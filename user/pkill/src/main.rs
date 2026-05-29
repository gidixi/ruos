#[link(wasm_import_module = "ruos")]
extern "C" {
    fn proc_list(buf_ptr: u32, buf_len: u32, used_ptr: u32) -> i32;
    fn proc_kill(pid: i32) -> i32;
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("pkill: usage: pkill <name-substring>");
        std::process::exit(1);
    }
    let pat = &args[0];
    let mut buf = vec![0u8; 8192];
    let mut used: u32 = 0;
    let errno = unsafe {
        proc_list(buf.as_mut_ptr() as u32, buf.len() as u32, &mut used as *mut u32 as u32)
    };
    if errno != 0 {
        eprintln!("pkill: proc_list errno {}", errno);
        std::process::exit(1);
    }
    let used = used as usize;
    if used < 4 {
        std::process::exit(1);
    }
    let count = u32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
    let mut o = 4usize;
    let mut killed = 0;
    for _ in 0..count {
        if o + 14 > used { break; }
        let pid = u32::from_le_bytes(buf[o..o + 4].try_into().unwrap()) as i32;
        let nl = u16::from_le_bytes(buf[o + 12..o + 14].try_into().unwrap()) as usize;
        let name_start = o + 16;
        if name_start + nl > used { break; }
        let name = std::str::from_utf8(&buf[name_start..name_start + nl]).unwrap_or("");
        if name.contains(pat.as_str()) {
            let _ = unsafe { proc_kill(pid) };
            killed += 1;
        }
        o = name_start + nl;
    }
    if killed == 0 { std::process::exit(1); }
}
