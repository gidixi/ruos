/// server.wasm — WASI Preview 1 socket activation model.
///
/// The kernel pre-opens a listening TCP socket at FD 4.
/// We call sock_accept to get the connected FD, then
/// fd_read / fd_write to exchange ping/pong.

fn main() {
    println!("server.wasm: listening on 127.0.0.1:8080");

    // FD 4 is the pre-opened listening socket provided by the kernel.
    const LISTEN_FD: u32 = 4;

    // sock_accept(listen_fd, flags, &accepted_fd) → errno
    let mut accepted_fd: u32 = u32::MAX;
    let errno = unsafe {
        wasi::sock_accept(LISTEN_FD, 0, &mut accepted_fd as *mut u32)
    };
    if errno != 0 {
        eprintln!("server.wasm: sock_accept failed errno={}", errno);
        std::process::exit(1);
    }
    println!("server.wasm: accepted fd={}", accepted_fd);

    // fd_read(fd, iovs_ptr, iovs_len, nread_ptr) → errno
    let mut buf = [0u8; 32];
    let mut nread: u32 = 0;
    let iov = WasiIov { buf: buf.as_mut_ptr(), len: buf.len() as u32 };
    let errno = unsafe {
        wasi::fd_read(accepted_fd, &iov as *const WasiIov, 1, &mut nread as *mut u32)
    };
    if errno != 0 {
        eprintln!("server.wasm: fd_read failed errno={}", errno);
        std::process::exit(1);
    }
    let received = core::str::from_utf8(&buf[..nread as usize]).unwrap_or("?");
    println!("server.wasm: rx='{}' tx='pong'", received);

    // fd_write(fd, iovs_ptr, iovs_len, nwritten_ptr) → errno
    let response = b"pong";
    let mut nwritten: u32 = 0;
    let iov2 = WasiIov { buf: response.as_ptr() as *mut u8, len: response.len() as u32 };
    let errno = unsafe {
        wasi::fd_write(accepted_fd, &iov2 as *const WasiIov, 1, &mut nwritten as *mut u32)
    };
    if errno != 0 {
        eprintln!("server.wasm: fd_write failed errno={}", errno);
        std::process::exit(1);
    }
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
        pub fn sock_accept(fd: u32, flags: u32, result_fd: *mut u32) -> u32;
        pub fn fd_read(fd: u32, iovs: *const super::WasiIov, iovs_len: u32, nread: *mut u32) -> u32;
        pub fn fd_write(fd: u32, iovs: *const super::WasiIov, iovs_len: u32, nwritten: *mut u32) -> u32;
    }
}
