//! Yield points for I/O host fns.
//!
//! A host function that needs to wait (sleep, socket I/O, VFS, etc.)
//! returns `Err(Error::host(SuspendReason::...))` instead of blocking.
//! `Fiber::run` catches this host error, awaits the matching async
//! future, writes any result bytes into wasm memory, then resumes the
//! call with errno=0 (or an errno on failure).

use alloc::vec::Vec;
use alloc::string::String;
use smoltcp::iface::SocketHandle;
use smoltcp::wire::IpEndpoint;

#[derive(Debug, Clone)]
pub enum SuspendReason {
    Sleep {
        ticks: u64,
        events_ptr: u32,
        nevents_ptr: u32,
    },
    SockAccept { handle: SocketHandle, new_fd_ptr: u32 },
    SockConnect { handle: SocketHandle, remote: IpEndpoint, local_port: u16 },
    SockRecv { handle: SocketHandle, buf_ptr: u32, max_len: usize, nrecv_ptr: u32 },
    SockSend { handle: SocketHandle, bytes: Vec<u8>, nsent_ptr: u32 },
    VfsRead { fd: crate::vfs::Fd, buf_ptr: u32, max_len: usize, nread_ptr: u32 },
    VfsWrite { fd: crate::vfs::Fd, bytes: Vec<u8>, nwritten_ptr: u32 },
    VfsSeek { fd: crate::vfs::Fd, offset: i64, whence: crate::vfs::Whence, newoffset_ptr: u32 },
    VfsClose { fd: crate::vfs::Fd },
    PathOpen { path: String, flags: crate::vfs::OpenFlags, opened_fd_ptr: u32 },
    Exec {
        path: alloc::string::String,
        argv: alloc::vec::Vec<alloc::vec::Vec<u8>>,
        cwd: alloc::string::String,
        /// PTY pair index of the calling shell so the spawned child's stdio
        /// inherits it instead of defaulting to /dev/pts/0. Without this,
        /// SSH-spawned shells run commands whose output lands on the boot
        /// framebuffer rather than the SSH channel.
        term_pts: usize,
        exit_code_ptr: u32,
    },
    ExecPipeline {
        stages: alloc::vec::Vec<(alloc::string::String, alloc::vec::Vec<alloc::vec::Vec<u8>>)>,
        cwd: alloc::string::String,
        /// PTY pair index of the calling shell, so the pipeline's terminal-
        /// facing FDs (first stdin, last stdout, all stderr) inherit it
        /// instead of defaulting to /dev/pts/0.
        term_pts: usize,
        exit_code_ptr: u32,
    },
    ReadDir {
        path: alloc::string::String,
        buf_ptr: u32,
        buf_len: usize,
        nread_ptr: u32,
    },
    PathUnlink   { path: String },
    PathMkdir    { path: String },
    PathRmdir    { path: String },
    PathFilestat { path: String, buf_ptr: u32 },
    PathRename   { src: String, dst: String },
    /// ICMPv4 echo (ping). `latency_ms_ptr` receives the round-trip in ms.
    Ping { target: smoltcp::wire::Ipv4Address, timeout_ticks: u64, latency_ms_ptr: u32 },
}

impl core::fmt::Display for SuspendReason {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl wasmi::errors::HostError for SuspendReason {}
