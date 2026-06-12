//! WASI Preview 1 on a `wasmtime::Linker<WtState>`. Enough to run real
//! `wasm32-wasip1` command tools that use argv/env, stdout, and read files.
//! VFS operations run synchronously via `crate::vfs::block_on` (tmpfs futures
//! complete in a single poll); stdout/stderr fan out to `crate::console::CONSOLE`
//! (serial + framebuffer). PTY/socket/blocking-stdin coverage is added later.

use wasmtime::{Caller, Linker};
use crate::wasm::wt::state::{WtState, WtFd, HasWasi};
use crate::wasm::wt::mem;
use crate::vfs;

const OK: i32 = 0;
const EBADF: i32 = 8;
const EINVAL: i32 = 28;
const EIO: i32 = 29;
const ENOTDIR: i32 = 54;
const ENOENT: i32 = 44;

pub fn add_to_linker<T: HasWasi + 'static>(linker: &mut Linker<T>) -> wasmtime::Result<()> {
    use wasmtime::Error;

    // proc_exit(code) -> !
    linker.func_wrap("wasi_snapshot_preview1", "proc_exit",
        |mut caller: Caller<'_, T>, code: i32| -> wasmtime::Result<()> {
            caller.data_mut().wasi().exit = Some(code);
            Err(Error::msg("proc_exit"))
        })?;

    // fd_write(fd, iovs, iovs_len, nwritten) -> errno. stdout/stderr only.
    linker.func_wrap("wasi_snapshot_preview1", "fd_write",
        |mut caller: Caller<'_, T>, fd: i32, iovs: i32, iovs_len: i32, nwritten: i32| -> i32 {
            if fd != 1 && fd != 2 { return EBADF; }
            let table = match mem::read(&mut caller, iovs as u32, (iovs_len as u32) * 8) {
                Some(t) => t, None => return EINVAL };
            let mut total: u32 = 0;
            for i in 0..iovs_len as usize {
                let b = i * 8;
                let ptr = u32::from_le_bytes(table[b..b+4].try_into().unwrap());
                let len = u32::from_le_bytes(table[b+4..b+8].try_into().unwrap());
                if len == 0 { continue; }
                let bytes = match mem::read(&mut caller, ptr, len) { Some(x) => x, None => return EINVAL };
                match caller.data().wasi_ref().stdout_pty {
                    Some(pfd) => { let _ = vfs::block_on(vfs::write(pfd, &bytes)); }
                    None => {
                        if let Ok(s) = core::str::from_utf8(&bytes) {
                            use core::fmt::Write as _;
                            let _ = crate::console::CONSOLE.lock().write_str(s);
                        }
                        // CONSOLE è solo serial+framebuffer — e col desktop su,
                        // il fb è coperto dalla GUI: su HW reale senza seriale
                        // lo stdout di un'app finestra sarebbe INVISIBILE.
                        // Spingilo anche nel ring dmesg e su netconsole.
                        crate::klog::push(&bytes);
                        #[cfg(feature = "netconsole")]
                        crate::net::netconsole::enqueue(&bytes);
                    }
                }
                total += len;
            }
            if !mem::write_u32(&mut caller, nwritten as u32, total) { return EINVAL; }
            OK
        })?;

    // fd_read(fd, iovs, iovs_len, nread) -> errno. VFS files via block_on.
    linker.func_wrap("wasi_snapshot_preview1", "fd_read",
        |mut caller: Caller<'_, T>, fd: i32, iovs: i32, iovs_len: i32, nread: i32| -> i32 {
            let vfd = match caller.data().wasi_ref().get(fd) {
                Some(WtFd::Vfs(f)) => *f,
                Some(WtFd::Console) => { // stdin → EOF
                    return if mem::write_u32(&mut caller, nread as u32, 0) { OK } else { EINVAL };
                }
                _ => return EBADF,
            };
            let table = match mem::read(&mut caller, iovs as u32, (iovs_len as u32) * 8) {
                Some(t) => t, None => return EINVAL };
            let mut total: u32 = 0;
            for i in 0..iovs_len as usize {
                let b = i * 8;
                let ptr = u32::from_le_bytes(table[b..b+4].try_into().unwrap());
                let len = u32::from_le_bytes(table[b+4..b+8].try_into().unwrap());
                if len == 0 { continue; }
                let mut buf = alloc::vec![0u8; len as usize];
                let n = match vfs::block_on(vfs::read(vfd, &mut buf)) { Ok(n) => n, Err(_) => return EIO };
                if n > 0 && !mem::write(&mut caller, ptr, &buf[..n]) { return EINVAL; }
                total += n as u32;
                if (n as u32) < len { break; } // short read = EOF for this call
            }
            if !mem::write_u32(&mut caller, nread as u32, total) { return EINVAL; }
            OK
        })?;

    // fd_seek(fd, offset, whence, newoffset) -> errno
    linker.func_wrap("wasi_snapshot_preview1", "fd_seek",
        |mut caller: Caller<'_, T>, fd: i32, offset: i64, whence: i32, newoff: i32| -> i32 {
            let vfd = match caller.data().wasi_ref().get(fd) { Some(WtFd::Vfs(f)) => *f, _ => return EBADF };
            let w = match whence { 0 => vfs::Whence::Set, 1 => vfs::Whence::Cur, 2 => vfs::Whence::End, _ => return EINVAL };
            let pos = match vfs::block_on(vfs::seek(vfd, offset, w)) { Ok(p) => p, Err(_) => return EIO };
            if !mem::write(&mut caller, newoff as u32, &pos.to_le_bytes()) { return EINVAL; }
            OK
        })?;

    // fd_close(fd) -> errno
    linker.func_wrap("wasi_snapshot_preview1", "fd_close",
        |mut caller: Caller<'_, T>, fd: i32| -> i32 {
            match caller.data().wasi_ref().get(fd) {
                Some(WtFd::Vfs(f)) => {
                    let f = *f;
                    let _ = vfs::block_on(vfs::close(f));
                    if let Some(slot) = caller.data_mut().wasi().fds.get_mut(fd as usize) { *slot = WtFd::Closed; }
                    OK
                }
                Some(WtFd::Console) => OK,
                _ => EBADF,
            }
        })?;

    // fd_fdstat_get(fd, out) -> errno. 24-byte struct; grant all rights.
    linker.func_wrap("wasi_snapshot_preview1", "fd_fdstat_get",
        |mut caller: Caller<'_, T>, fd: i32, out: i32| -> i32 {
            let filetype: u8 = if fd == 3 {
                3 // preopen dir
            } else {
                match caller.data().wasi_ref().get(fd) {
                    Some(WtFd::Console) => 2, // char device
                    Some(WtFd::Vfs(f)) => match vfs::block_on(vfs::stat_fd(*f)) {
                        Ok(s) => kind_to_filetype(s.kind), Err(_) => 0,
                    },
                    _ => return EBADF,
                }
            };
            let mut st = [0u8; 24];
            st[0] = filetype;
            st[8..16].copy_from_slice(&u64::MAX.to_le_bytes());
            st[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
            if !mem::write(&mut caller, out as u32, &st) { return EINVAL; }
            OK
        })?;

    // fd_filestat_get(fd, out) -> errno. 64-byte struct (filetype@16, size@32).
    linker.func_wrap("wasi_snapshot_preview1", "fd_filestat_get",
        |mut caller: Caller<'_, T>, fd: i32, out: i32| -> i32 {
            let (ft, size): (u8, u64) = match caller.data().wasi_ref().get(fd) {
                Some(WtFd::Console) => (2, 0),
                Some(WtFd::Vfs(f)) => match vfs::block_on(vfs::stat_fd(*f)) {
                    Ok(s) => (kind_to_filetype(s.kind), s.size), Err(_) => return EIO,
                },
                _ => return EBADF,
            };
            let mut st = [0u8; 64];
            st[16] = ft;
            st[32..40].copy_from_slice(&size.to_le_bytes());
            if !mem::write(&mut caller, out as u32, &st) { return EINVAL; }
            OK
        })?;

    // path_filestat_get(dirfd, flags, path, path_len, out) -> errno. 64-byte filestat
    // (filetype@16, size@32), same layout as fd_filestat_get. Resolves `path` against
    // the single "/" preopen like path_open. std::fs::metadata / Path::exists import
    // this; without it Rust-std guests (Blitz, and the kernel's own cat.cwasm) fail to
    // instantiate with "unknown import path_filestat_get". Best-effort via vfs::stat.
    linker.func_wrap("wasi_snapshot_preview1", "path_filestat_get",
        |mut caller: Caller<'_, T>, _dirfd: i32, _flags: i32, path: i32, path_len: i32, out: i32| -> i32 {
            let raw = match mem::read(&mut caller, path as u32, path_len as u32) { Some(b) => b, None => return EINVAL };
            let rel = match core::str::from_utf8(&raw) { Ok(s) => s, Err(_) => return EINVAL };
            let abs = alloc::format!("/{}", rel.trim_start_matches('/'));
            let (ft, size): (u8, u64) = match vfs::block_on(vfs::stat(&abs)) {
                Ok(s) => (kind_to_filetype(s.kind), s.size),
                Err(_) => return ENOENT,
            };
            let mut st = [0u8; 64];
            st[16] = ft;
            st[32..40].copy_from_slice(&size.to_le_bytes());
            if !mem::write(&mut caller, out as u32, &st) { return EINVAL; }
            OK
        })?;

    // fd_prestat_get(fd, out) -> errno. Only fd 3 = preopen "/".
    linker.func_wrap("wasi_snapshot_preview1", "fd_prestat_get",
        |mut caller: Caller<'_, T>, fd: i32, out: i32| -> i32 {
            if fd != 3 { return EBADF; }
            let mut st = [0u8; 8];
            st[0..4].copy_from_slice(&0u32.to_le_bytes()); // PREOPENTYPE_DIR
            st[4..8].copy_from_slice(&1u32.to_le_bytes()); // name_len = 1 ("/")
            if !mem::write(&mut caller, out as u32, &st) { return EINVAL; }
            OK
        })?;

    // fd_prestat_dir_name(fd, path, path_len) -> errno
    linker.func_wrap("wasi_snapshot_preview1", "fd_prestat_dir_name",
        |mut caller: Caller<'_, T>, fd: i32, path: i32, _len: i32| -> i32 {
            if fd != 3 { return EBADF; }
            if !mem::write(&mut caller, path as u32, b"/") { return EINVAL; }
            OK
        })?;

    // path_open(dirfd, dirflags, path, path_len, oflags, rights_base,
    //           rights_inheriting, fdflags, opened_fd) -> errno
    linker.func_wrap("wasi_snapshot_preview1", "path_open",
        |mut caller: Caller<'_, T>, _dirfd: i32, _dirflags: i32, path: i32, path_len: i32,
         oflags: i32, _rb: i64, _ri: i64, _fdflags: i32, opened: i32| -> i32 {
            let raw = match mem::read(&mut caller, path as u32, path_len as u32) { Some(b) => b, None => return EINVAL };
            let rel = match core::str::from_utf8(&raw) { Ok(s) => s, Err(_) => return EINVAL };
            let abs = alloc::format!("/{}", rel.trim_start_matches('/'));
            // O_DIRECTORY (1<<1): verify it's a directory; we expose dir fds as
            // VFS-less handles only for readdir (not implemented here) → ENOTDIR
            // path uses fd_read, so reject directory opens for now unless it's a file.
            if oflags & (1 << 1) != 0 {
                return match vfs::block_on(vfs::stat(&abs)) {
                    Ok(s) if s.kind == vfs::VfsKind::Dir => ENOTDIR, // dir fds (readdir) TODO
                    Ok(_) => ENOTDIR,
                    Err(_) => ENOENT,
                };
            }
            let mut flags = vfs::OpenFlags::READ | vfs::OpenFlags::WRITE;
            if oflags & (1 << 0) != 0 { flags |= vfs::OpenFlags::CREATE; }
            if oflags & (1 << 3) != 0 { flags |= vfs::OpenFlags::TRUNCATE; }
            let vfd = match vfs::block_on(vfs::open(&abs, flags)) {
                Ok(f) => f,
                Err(_) => {
                    // Retry read-only (e.g. read-only files).
                    match vfs::block_on(vfs::open(&abs, vfs::OpenFlags::READ)) {
                        Ok(f) => f,
                        Err(e) => { crate::kprintln!("ruos: wt path_open '{}' err {:?}", abs, e); return ENOENT; }
                    }
                }
            };
            let fd = caller.data_mut().wasi().install_vfs(vfd);
            if !mem::write_u32(&mut caller, opened as u32, fd as u32) { return EINVAL; }
            OK
        })?;

    // args / environ.
    linker.func_wrap("wasi_snapshot_preview1", "args_sizes_get",
        |mut caller: Caller<'_, T>, argc: i32, buf_size: i32| -> i32 {
            let n = caller.data().wasi_ref().args.len() as u32;
            let sz: u32 = caller.data().wasi_ref().args.iter().map(|a| a.len() as u32 + 1).sum();
            if !mem::write_u32(&mut caller, argc as u32, n) { return EINVAL; }
            if !mem::write_u32(&mut caller, buf_size as u32, sz) { return EINVAL; }
            OK
        })?;
    linker.func_wrap("wasi_snapshot_preview1", "args_get",
        |mut caller: Caller<'_, T>, argv: i32, buf: i32| -> i32 {
            let args = caller.data().wasi_ref().args.clone();
            let mut cursor = buf as u32;
            for (i, arg) in args.iter().enumerate() {
                let slot = argv as u32 + (i as u32) * 4;
                if !mem::write_u32(&mut caller, slot, cursor) { return EINVAL; }
                let mut owned = arg.clone();
                owned.push(0);
                if !mem::write(&mut caller, cursor, &owned) { return EINVAL; }
                cursor += owned.len() as u32;
            }
            OK
        })?;
    // environ: same layout contract as args_* above, sourced from WtState.env
    // ("K=V" entries). Empty for classic tools (count 0, as the old stubs);
    // threaded modules see RAYON_NUM_THREADS injected by threads::exec_threaded.
    linker.func_wrap("wasi_snapshot_preview1", "environ_sizes_get",
        |mut caller: Caller<'_, T>, c: i32, s: i32| -> i32 {
            let n = caller.data().wasi_ref().env.len() as u32;
            let sz: u32 = caller.data().wasi_ref().env.iter().map(|e| e.len() as u32 + 1).sum();
            if !mem::write_u32(&mut caller, c as u32, n) { return EINVAL; }
            if !mem::write_u32(&mut caller, s as u32, sz) { return EINVAL; }
            OK
        })?;
    linker.func_wrap("wasi_snapshot_preview1", "environ_get",
        |mut caller: Caller<'_, T>, environ: i32, buf: i32| -> i32 {
            let env = caller.data().wasi_ref().env.clone();
            let mut cursor = buf as u32;
            for (i, entry) in env.iter().enumerate() {
                let slot = environ as u32 + (i as u32) * 4;
                if !mem::write_u32(&mut caller, slot, cursor) { return EINVAL; }
                let mut owned = entry.clone();
                owned.push(0);
                if !mem::write(&mut caller, cursor, &owned) { return EINVAL; }
                cursor += owned.len() as u32;
            }
            OK
        })?;

    // clock_time_get(id, precision, time_out) -> errno.
    // id 0 (REALTIME): unix-epoch ns, anchored to the RTC once at first use —
    // TLS in-app (rustls) validates certificate windows against SystemTime,
    // so this must be wall-clock, not uptime. Other ids: monotonic-ish ns from
    // the 100 Hz timer (10 ms/tick), good enough for std/egui bookkeeping.
    linker.func_wrap("wasi_snapshot_preview1", "clock_time_get",
        |mut caller: Caller<'_, T>, id: i32, _prec: i64, out: i32| -> i32 {
            let mono: u64 = crate::timer::ticks().wrapping_mul(10_000_000);
            let ns = if id == 0 { boot_epoch_ns().wrapping_add(mono) } else { mono };
            if mem::write(&mut caller, out as u32, &ns.to_le_bytes()) { OK } else { EINVAL }
        })?;

    // random_get(buf, len) -> errno. Fills from the kernel CSPRNG.
    linker.func_wrap("wasi_snapshot_preview1", "random_get",
        |mut caller: Caller<'_, T>, buf: i32, len: i32| -> i32 {
            if len < 0 { return EINVAL; }
            let mut tmp = alloc::vec![0u8; len as usize];
            crate::rng::fill(&mut tmp);
            if mem::write(&mut caller, buf as u32, &tmp) { OK } else { EINVAL }
        })?;

    // sched_yield() -> errno. Cooperative single-core: nothing to do.
    linker.func_wrap("wasi_snapshot_preview1", "sched_yield",
        |_caller: Caller<'_, T>| -> i32 { OK })?;

    // poll_oneoff(in, out, nsubs, nevents) -> errno. Clock subscriptions only
    // (sleep/timeout — what std::thread::sleep and C usleep/nanosleep emit),
    // mirroring the wasmi shim (host/lifecycle.rs). Non-clock subs → EINVAL.
    // Only the FIRST subscription is honored (std/libc sleep emit exactly one).
    //
    // Wait strategy: on a wasm-thread FIBER the sleep parks the fiber
    // (threads::sleep_current → expire_timeouts redeems it) so the core stays
    // free; on the classic sync .cwasm path it hlt-waits on the spot — that
    // blocks this core like any long-running sync tool (on 1-2 core systems
    // prefer the wasmi `.wasm` build of a sleepy CLI tool: wasmi suspends).
    linker.func_wrap("wasi_snapshot_preview1", "poll_oneoff",
        |mut caller: Caller<'_, T>, in_ptr: i32, out_ptr: i32, nsubs: i32, nevents: i32| -> i32 {
            if nsubs < 1 { return EINVAL; }
            // __wasi_subscription_t (48 bytes): userdata u64 @0, tag u16 @8
            // (0 = CLOCK), clock_id u32 @16, timeout u64 @24 (ns),
            // precision u64 @32, flags u16 @40 (bit0 = ABSTIME).
            let sub = match mem::read(&mut caller, in_ptr as u32, 48) {
                Some(s) => s, None => return EINVAL };
            let sub_type = u16::from_le_bytes([sub[8], sub[9]]);
            if sub_type != 0 { return EINVAL; } // solo clock (come wasmi)
            let mut userdata = [0u8; 8];
            userdata.copy_from_slice(&sub[0..8]);
            let timeout_ns = u64::from_le_bytes(sub[24..32].try_into().unwrap());
            let abstime = u16::from_le_bytes([sub[40], sub[41]]) & 1 != 0;

            const TICK_NS: u64 = 10_000_000; // 100 Hz
            let now = crate::timer::ticks();
            let target = if abstime {
                // ABSTIME su monotonic = ns dal boot (clock_time_get id != 0).
                (timeout_ns / TICK_NS).max(now)
            } else {
                now.saturating_add((timeout_ns + TICK_NS - 1) / TICK_NS)
            };
            if target > now {
                // Fiber (app threaded): park con deadline, il core resta libero.
                if !crate::wasm::wt::threads::sleep_current(target) {
                    // Path sync classico: hlt fino alla deadline (timer 100 Hz
                    // risveglia ogni tick; IRQ abilitati nel contesto exec).
                    while crate::timer::ticks() < target {
                        x86_64::instructions::hlt();
                    }
                }
            }
            // Un evento clock (__wasi_event_t, 32 bytes): userdata della sub,
            // error u16 = 0, type u8 = 0 (CLOCK), resto zero.
            let mut event = [0u8; 32];
            event[0..8].copy_from_slice(&userdata);
            if !mem::write(&mut caller, out_ptr as u32, &event) { return EINVAL; }
            if !mem::write_u32(&mut caller, nevents as u32, 1) { return EINVAL; }
            OK
        })?;

    Ok(())
}

/// Unix-epoch ns at boot (epoch_now − monotonic_now), cached on first call so
/// the CMOS RTC is read once, not on every guest `SystemTime::now()`.
fn boot_epoch_ns() -> u64 {
    use core::sync::atomic::{AtomicU64, Ordering};
    static BOOT_EPOCH_NS: AtomicU64 = AtomicU64::new(0);
    let mut v = BOOT_EPOCH_NS.load(Ordering::Relaxed);
    if v == 0 {
        let epoch_s = crate::rtc::to_unix_epoch(&crate::rtc::now());
        let mono = crate::timer::ticks().wrapping_mul(10_000_000);
        v = epoch_s.wrapping_mul(1_000_000_000).wrapping_sub(mono);
        BOOT_EPOCH_NS.store(v, Ordering::Relaxed);
    }
    v
}

fn kind_to_filetype(k: vfs::VfsKind) -> u8 {
    match k {
        vfs::VfsKind::Reg => 4,    // REGULAR_FILE
        vfs::VfsKind::Dir => 3,    // DIRECTORY
        vfs::VfsKind::Device => 2, // CHARACTER_DEVICE
    }
}
