#[link(wasm_import_module = "ruos")]
extern "C" {
    fn usb_list(buf_ptr: u32, buf_len: u32, used_ptr: u32) -> i32;
}

fn main() {
    let mut buf = vec![0u8; 4096];
    let mut used: u32 = 0;
    let errno = unsafe {
        usb_list(
            buf.as_mut_ptr() as u32,
            buf.len() as u32,
            &mut used as *mut u32 as u32,
        )
    };
    if errno == 8 {
        // ENOBUFS: grow buffer to required size and retry once.
        buf = vec![0u8; used as usize];
        let errno = unsafe {
            usb_list(
                buf.as_mut_ptr() as u32,
                buf.len() as u32,
                &mut used as *mut u32 as u32,
            )
        };
        if errno != 0 {
            eprintln!("lsusb: usb_list errno {}", errno);
            std::process::exit(1);
        }
    } else if errno != 0 {
        eprintln!("lsusb: usb_list errno {}", errno);
        std::process::exit(1);
    }
    let text = match core::str::from_utf8(&buf[..used as usize]) {
        Ok(s) => s,
        Err(_) => { eprintln!("lsusb: invalid utf-8"); std::process::exit(1); }
    };
    print!("{}", text);
}
