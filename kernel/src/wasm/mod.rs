//! Wasmi 1.x async fiber runtime for ruos.
//! Task 3: Runtime struct removed; all wasm execution goes through Fiber.

pub mod host;
pub mod state;
pub mod suspend;
pub mod fiber;
pub mod exec_queue;
pub mod pipeline;
pub mod ssh_spawn;
pub mod wt;

use alloc::vec::Vec;
use crate::kprintln;
use crate::vfs;

/// Spawn `/bin/shell.wasm` on the default console PTY (pts/0) and run it to
/// completion, returning its exit code. `replay_init` controls whether the
/// shell replays `/etc/init.sh` + banner (true only on the very first boot
/// launch; false on every respawn after the user types `exit`, so a fresh
/// prompt appears without re-running the boot script). The local console is
/// thus never left dead — same idea as `init`/getty respawn on Unix.
pub async fn run_boot_shell(replay_init: bool) -> i32 {
    const PATH: &str = "/bin/shell.wasm";
    let bytes = match read_all(PATH).await {
        Ok(b) => b,
        Err(e) => {
            kprintln!("ruos: boot shell: read {} failed: {:?}", PATH, e);
            return 127;
        }
    };
    let mut fb = match crate::wasm::fiber::Fiber::new(&bytes) {
        Ok(f) => f,
        Err(e) => {
            kprintln!("ruos: boot shell: instantiate failed: {}", e);
            return 126;
        }
    };
    if replay_init {
        // argv = ["/bin/shell.wasm"] → shell replays /etc/init.sh.
        fb.set_args(alloc::vec![PATH.as_bytes().to_vec()]);
    } else {
        // argv = ["shell", "--no-init"] → skip the boot script, fresh prompt.
        fb.set_args(alloc::vec![b"shell".to_vec(), b"--no-init".to_vec()]);
    }
    let pid = crate::proc::register(alloc::string::String::from(PATH.trim_start_matches('/')));
    fb.set_pid(pid);
    let code = fb.run().await;
    crate::proc::unregister(pid);
    code
}

#[allow(dead_code)] // kept for /root/{server,client}.wasm demo blobs + future use
pub async fn run_at(path: &str) {
    let bytes = match read_all(path).await {
        Ok(b) => b,
        Err(e) => {
            kprintln!("ruos: wasm: read {} failed: {:?}", path, e);
            return;
        }
    };

    crate::wtrace!("ruos: wasm: about to instantiate {}", path);
    let mut fb = match crate::wasm::fiber::Fiber::new(&bytes) {
        Ok(f) => f,
        Err(e) => {
            kprintln!("ruos: wasm: instantiate {} failed: {}", path, e);
            return;
        }
    };
    crate::wtrace!("ruos: wasm: instantiated {}", path);
    fb.set_args(alloc::vec![path.as_bytes().to_vec()]);
    let pid = crate::proc::register(alloc::string::String::from(path.trim_start_matches('/')));
    fb.set_pid(pid);

    // Pre-open socket FD 4 for server and client.
    // Server: allocate + listen (sync instant); cooperative accept happens in fiber dispatch.
    // Client: allocate + async connect (yields until Established, then inject FD 4).
    match path {
        "/root/server.wasm" => {
            let idx = crate::net::sockets::POOL.alloc_tcp();
            let handle = crate::net::sockets::POOL.handle(idx).expect("server socket");
            crate::net::sockets::listen(handle, 8080).expect("listen");
            crate::wtrace!("ruos: server socket listening port=8080 idx={}", idx);
            let fds = &mut fb.store.data_mut().fds;
            if fds.len() <= 4 {
                fds.resize_with(5, || None);
            }
            fds[4] = Some(crate::wasm::state::FdEntry::Socket(idx));
        }
        "/root/client.wasm" => {
            use smoltcp::wire::{IpAddress, IpEndpoint};
            let idx = crate::net::sockets::POOL.alloc_tcp();
            let handle = crate::net::sockets::POOL.handle(idx).expect("client socket");
            let remote = IpEndpoint::new(IpAddress::v4(127, 0, 0, 1), 8080);
            crate::wtrace!("ruos: client socket connecting idx={}", idx);
            match crate::net::sockets::connect(handle, remote, 49152).await {
                Ok(()) => crate::wtrace!("ruos: client socket connected idx={}", idx),
                Err(e) => {
                    kprintln!("ruos: client socket connect failed: {}", e);
                    return;
                }
            }
            let fds = &mut fb.store.data_mut().fds;
            if fds.len() <= 4 {
                fds.resize_with(5, || None);
            }
            fds[4] = Some(crate::wasm::state::FdEntry::Socket(idx));
        }
        _ => {}
    }

    let code = fb.run().await;
    crate::proc::unregister(pid);
    let short = path.trim_start_matches('/');
    if code == 0 {
        kprintln!("ruos: {} exited cleanly", short);
    } else {
        kprintln!("ruos: {} exited code={}", short, code);
    }
}

pub(crate) async fn read_all(path: &str) -> Result<Vec<u8>, vfs::VfsError> {
    let fd = vfs::open(path, vfs::OpenFlags::READ).await?;
    // Seek to end to find size; then seek back to start and read.
    let end = vfs::seek(fd, 0, vfs::Whence::End).await? as usize;
    vfs::seek(fd, 0, vfs::Whence::Set).await?;
    let mut buf = alloc::vec![0u8; end];
    let mut read = 0;
    while read < end {
        let n = vfs::read(fd, &mut buf[read..]).await?;
        if n == 0 {
            break;
        }
        read += n;
    }
    vfs::close(fd).await?;
    Ok(buf)
}
