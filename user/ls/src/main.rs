#[link(wasm_import_module = "ruos")]
extern "C" {
    fn readdir(
        path_ptr: u32, path_len: u32,
        buf_ptr: u32, buf_len: u32,
        nread_ptr: u32,
    ) -> i32;
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // No-arg ls = current directory ("." resolves via kernel's per-fiber
    // CWD which is inherited from the shell via ruos_exec).
    let path = args.get(1).cloned().unwrap_or_else(|| ".".to_string());
    let mut buf = vec![0u8; 8192];
    let mut nread: u32 = 0;
    let errno = unsafe {
        readdir(
            path.as_ptr() as u32, path.len() as u32,
            buf.as_mut_ptr() as u32, buf.len() as u32,
            &mut nread as *mut u32 as u32,
        )
    };
    if errno != 0 {
        eprintln!("ls: {}: errno {}", path, errno);
        std::process::exit(1);
    }
    let mut offset = 0usize;
    while offset + 12 <= nread as usize {
        let kind = buf[offset];
        let name_len = u16::from_le_bytes([buf[offset + 2], buf[offset + 3]]) as usize;
        let size = u64::from_le_bytes([
            buf[offset + 4], buf[offset + 5], buf[offset + 6], buf[offset + 7],
            buf[offset + 8], buf[offset + 9], buf[offset + 10], buf[offset + 11],
        ]);
        offset += 12;
        if offset + name_len > nread as usize { break; }
        let name = match std::str::from_utf8(&buf[offset..offset + name_len]) {
            Ok(s) => s,
            Err(_) => "?",
        };
        offset += name_len;
        let kind_str = match kind { 0 => "REG", 1 => "DIR", 2 => "DEV", _ => "???" };
        let mark = match kind { 1 => "/", 2 => "@", _ => "" };
        println!("{} {:>8} {}{}", kind_str, size, name, mark);
    }
}
