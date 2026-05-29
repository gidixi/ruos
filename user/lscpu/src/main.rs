#[link(wasm_import_module = "ruos")]
extern "C" {
    fn cpuinfo(buf_ptr: u32, buf_len: u32, used_ptr: u32) -> i32;
}

fn main() {
    let mut buf = vec![0u8; 256];
    let mut used: u32 = 0;
    let errno = unsafe {
        cpuinfo(buf.as_mut_ptr() as u32, buf.len() as u32, &mut used as *mut u32 as u32)
    };
    if errno != 0 {
        eprintln!("lscpu: errno {}", errno);
        std::process::exit(1);
    }
    let parts: Vec<&str> = buf[..used as usize]
        .split(|b| *b == 0)
        .map(|s| std::str::from_utf8(s).unwrap_or("?"))
        .collect();
    let vendor = parts.get(0).copied().unwrap_or("");
    let brand  = parts.get(1).copied().unwrap_or("");
    let ncpu   = parts.get(2).copied().unwrap_or("1");
    println!("Architecture:    x86_64");
    println!("CPU(s):          {}", ncpu);
    println!("Vendor ID:       {}", vendor.trim_matches(char::from(0)));
    println!("Model name:      {}", brand.trim_matches(char::from(0)));
}
