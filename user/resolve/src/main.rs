//! resolve — DNS lookup tool
//!
//! Usage:
//!   resolve <hostname>

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn net_resolve(
        name_ptr: i32,
        name_len: i32,
        addrs_out_ptr: i32,
        max_addrs: i32,
        count_out_ptr: i32,
    ) -> i32;
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: resolve <hostname>");
        std::process::exit(2);
    }
    let name = &args[0];

    // Max 16 IPv4 addresses
    let mut addrs = [0u8; 16 * 4];
    let mut count = 0u32;

    let r = unsafe {
        net_resolve(
            name.as_ptr() as i32,
            name.len() as i32,
            addrs.as_mut_ptr() as i32,
            16,
            &mut count as *mut u32 as i32,
        )
    };

    if r != 0 {
        eprintln!("resolve: failed to resolve '{}' (errno {})", name, r);
        std::process::exit(1);
    }

    if count == 0 {
        println!("resolve: no addresses found for '{}'", name);
    } else {
        for i in 0..count as usize {
            let offset = i * 4;
            let ip = &addrs[offset..offset + 4];
            println!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
        }
    }
}
