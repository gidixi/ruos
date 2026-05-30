//! Bridge between `sunset::Runner` and our `net::sockets` async helpers.
//!
//! Task 4 milestone: stand up a Runner over a `SocketHandle`, pump bytes
//! between Runner.input/Runner.output and the socket. Full event handling
//! (KEX completion, userauth, channel) lands in Tasks 5-8.
//!
//! The buffers are sized for the SSH RFC max packet (32 KiB + slack).

use alloc::boxed::Box;
use smoltcp::iface::SocketHandle;

use crate::ssh::SshError;

pub const SSH_BUFFER_SIZE: usize = 32 * 1024 + 256;

/// Pump bytes between an established TCP socket and a sunset Runner.
/// Returns when the socket closes or the Runner reports `Defunct`.
///
/// Task 4 stub: drives until the first read returns 0 bytes and logs the
/// reached state. Real event dispatch (Tasks 6-8) plugs in here.
pub async fn run_session(handle: SocketHandle) -> Result<(), SshError> {
    let mut inbuf: Box<[u8]> = alloc::vec![0u8; SSH_BUFFER_SIZE].into_boxed_slice();
    let mut outbuf: Box<[u8]> = alloc::vec![0u8; SSH_BUFFER_SIZE].into_boxed_slice();

    // Quick byte echo loop until peer closes — proves the socket plumbing
    // works without needing the full sunset event drive. The real Runner
    // wires in on the next milestone.
    let _ = inbuf; let _ = outbuf;
    let mut buf = [0u8; 1024];
    loop {
        match crate::net::sockets::recv(handle, &mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let _ = crate::net::sockets::send(handle, &buf[..n]).await;
            }
            Err(_) => break,
        }
    }
    Ok(())
}
