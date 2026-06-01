#[link(wasm_import_module = "ruos")]
extern "C" {
    fn smp_bench(buf_ptr: u32, buf_len: u32, used_ptr: u32) -> i32;
}

fn main() {
    let mut buf = vec![0u8; 256];
    let mut used: u32 = 0;
    let errno = unsafe {
        smp_bench(buf.as_mut_ptr() as u32, buf.len() as u32, &mut used as *mut u32 as u32)
    };
    if errno != 0 {
        eprintln!("smptest: errno {}", errno);
        std::process::exit(1);
    }
    let report = std::str::from_utf8(&buf[..used as usize]).unwrap_or("?");
    println!("{}", report);
}
