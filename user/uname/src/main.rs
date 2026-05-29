#[link(wasm_import_module = "ruos")]
extern "C" {
    fn uname(buf_ptr: u32, buf_len: u32, used_ptr: u32) -> i32;
}

fn main() {
    let mut buf = vec![0u8; 256];
    let mut used: u32 = 0;
    let errno = unsafe {
        uname(buf.as_mut_ptr() as u32, buf.len() as u32, &mut used as *mut u32 as u32)
    };
    if errno != 0 {
        eprintln!("uname: errno {}", errno);
        std::process::exit(1);
    }
    let parts: Vec<&str> = buf[..used as usize]
        .split(|b| *b == 0)
        .map(|s| std::str::from_utf8(s).unwrap_or("?"))
        .collect();
    // parts: [name, node, release, version, machine]
    let args: Vec<String> = std::env::args().collect();
    let all = args.iter().any(|a| a == "-a");
    if all {
        println!("{} {} {} {} {}",
            parts.get(0).unwrap_or(&""),
            parts.get(1).unwrap_or(&""),
            parts.get(2).unwrap_or(&""),
            parts.get(3).unwrap_or(&""),
            parts.get(4).unwrap_or(&""));
    } else {
        println!("{}", parts.get(0).unwrap_or(&""));
    }
}
