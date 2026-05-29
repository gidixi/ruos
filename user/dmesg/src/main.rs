#[link(wasm_import_module = "ruos")]
extern "C" {
    fn dmesg(buf_ptr: u32, buf_len: u32, used_ptr: u32) -> i32;
}

fn main() {
    let mut buf = vec![0u8; 32 * 1024];
    let mut used: u32 = 0;
    let errno = unsafe {
        dmesg(buf.as_mut_ptr() as u32, buf.len() as u32, &mut used as *mut u32 as u32)
    };
    if errno != 0 {
        eprintln!("dmesg: errno {}", errno);
        std::process::exit(1);
    }
    let n = (used as usize).min(buf.len());
    if let Ok(s) = std::str::from_utf8(&buf[..n]) {
        print!("{}", s);
    } else {
        let s = String::from_utf8_lossy(&buf[..n]);
        print!("{}", s);
    }
}
