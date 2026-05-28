/// client.wasm — WASI Preview 1 socket activation model.
///
/// The kernel pre-opens a connected TCP socket at FD 4
/// (connected to the server's listening socket).
/// We write "ping" then read "pong".

fn main() {
    // FD 4 is the pre-opened connected socket provided by the kernel.
    const SOCKET_FD: u32 = 4;

    // fd_write → send "ping"
    let msg = b"ping";
    let mut nwritten: u32 = 0;
    let iov = WasiIov { buf: msg.as_ptr() as *mut u8, len: msg.len() as u32 };
    let errno = unsafe {
        wasi::fd_write(SOCKET_FD, &iov as *const WasiIov, 1, &mut nwritten as *mut u32)
    };
    if errno != 0 {
        eprintln!("client.wasm: fd_write failed errno={}", errno);
        std::process::exit(1);
    }
    println!("client.wasm: tx='ping'");

    // fd_read → receive "pong"
    let mut buf = [0u8; 32];
    let mut nread: u32 = 0;
    let iov2 = WasiIov { buf: buf.as_mut_ptr(), len: buf.len() as u32 };
    let errno = unsafe {
        wasi::fd_read(SOCKET_FD, &iov2 as *const WasiIov, 1, &mut nread as *mut u32)
    };
    if errno != 0 {
        eprintln!("client.wasm: fd_read failed errno={}", errno);
        std::process::exit(1);
    }
    let received = core::str::from_utf8(&buf[..nread as usize]).unwrap_or("?");
    println!("client.wasm: rx='{}'", received);
}

/// WASI iov (pointer + length pair), matching wasi_snapshot_preview1 layout.
#[repr(C)]
struct WasiIov {
    buf: *mut u8,
    len: u32,
}

mod wasi {
    #[link(wasm_import_module = "wasi_snapshot_preview1")]
    extern "C" {
        pub fn fd_read(fd: u32, iovs: *const super::WasiIov, iovs_len: u32, nread: *mut u32) -> u32;
        pub fn fd_write(fd: u32, iovs: *const super::WasiIov, iovs_len: u32, nwritten: *mut u32) -> u32;
    }
}
