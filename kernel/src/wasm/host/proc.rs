//! Custom (non-WASIX) host fns: ruos_exec + ruos_readdir + introspection.

use wasmi::{Caller, Linker, Error};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;
use crate::wasm::state::RuntimeState;
use crate::wasm::suspend::SuspendReason;

pub fn ruos_exec(
    caller: Caller<'_, RuntimeState>,
    path_ptr: i32,
    path_len: i32,
    argv_ptr: i32,
    argv_len: i32,
    exit_code_ptr: i32,
) -> Result<i32, Error> {
    let path_buf = match crate::wasm::host::mem::guest_read(&caller, path_ptr, path_len) {
        Ok(b) => b,
        Err(e) => return Ok(e),
    };
    let path = core::str::from_utf8(&path_buf)
        .map_err(|_| Error::i32_exit(-1))?
        .to_string();
    let path = resolve_cwd(&caller.data().cwd, &path);
    if !caller.data().grants(&path) { return Ok(76); } // ENOTCAPABLE
    let argv_blob = match crate::wasm::host::mem::guest_read(&caller, argv_ptr, argv_len) {
        Ok(b) => b,
        Err(e) => return Ok(e),
    };
    let argv = decode_argv(&argv_blob).unwrap_or_default();
    // Child inherits parent's CWD + terminal (PTY index). Same rationale as
    // ruos_exec_pipeline: SSH-spawned shells must hand their PTY to children
    // so command output reaches the SSH channel, not /dev/pts/0.
    let cwd = caller.data().cwd.clone();
    let term_pts = match caller.data().fds.get(1).and_then(|s| s.as_ref()) {
        Some(crate::wasm::state::FdEntry::Vfs(kfd)) => {
            crate::vfs::fd::pts_index(*kfd).unwrap_or(0)
        }
        _ => 0,
    };
    Err(Error::host(SuspendReason::Exec {
        path,
        argv,
        cwd,
        term_pts,
        exit_code_ptr: exit_code_ptr as u32,
    }))
}

/// ruos_chdir(path_ptr, path_len) -> errno
///
/// Updates the caller's CWD. Path may be relative — resolved against
/// the current CWD. Validates that the target exists and is a
/// directory before updating; returns ENOENT (44) if missing,
/// ENOTDIR (54) if it's a regular/device file.
pub fn ruos_chdir(
    mut caller: Caller<'_, RuntimeState>,
    path_ptr: i32,
    path_len: i32,
) -> Result<i32, Error> {
    let path_buf = match crate::wasm::host::mem::guest_read(&caller, path_ptr, path_len) {
        Ok(b) => b,
        Err(e) => return Ok(e),
    };
    let path = core::str::from_utf8(&path_buf)
        .map_err(|_| Error::i32_exit(-1))?;
    let new_cwd = resolve_cwd(&caller.data().cwd, path);
    // Capability check: reject targets outside the task's grant.
    if !caller.data().grants(&new_cwd) { return Ok(76); } // ENOTCAPABLE
    // Root always exists; skip stat to avoid a corner case.
    if new_cwd != "/" {
        match crate::vfs::block_on(crate::vfs::stat(&new_cwd)) {
            Ok(s) if matches!(s.kind, crate::vfs::VfsKind::Dir) => {}
            Ok(_) => return Ok(54),  // ENOTDIR
            Err(_) => return Ok(44), // ENOENT
        }
    }
    caller.data_mut().cwd = new_cwd;
    Ok(0)
}

/// ruos_umount(path_ptr, path_len) -> status.
///
/// Unmount the filesystem mounted at exactly `path` (e.g. "/mnt"). Dropping the
/// FsImpl releases its backing device (the SATA port) once no open file still
/// holds a ref to it — which is what lets `install` proceed onto a disk that
/// M1 auto-mounted (the install /mnt guard otherwise refuses). Refuses "/".
///
/// Returns 0 on success, -2 when the path cannot be unmounted (e.g. "/"),
/// -3 when a file is still open on the mount (busy — close it first),
/// -1 when nothing is mounted there (NotFound) or the path is unreadable.
fn ruos_umount(caller: Caller<'_, RuntimeState>, path_ptr: i32, path_len: i32) -> Result<i32, Error> {
    let buf = match crate::wasm::host::mem::guest_read(&caller, path_ptr, path_len) {
        Ok(b) => b,
        Err(_) => return Ok(-1),
    };
    let path = match core::str::from_utf8(&buf) {
        Ok(s) => s,
        Err(_) => return Ok(-1),
    };
    match crate::vfs::unmount(path) {
        Ok(())                                 => Ok(0),
        Err(crate::vfs::VfsError::InvalidPath) => Ok(-2),  // refused (e.g. "/")
        Err(crate::vfs::VfsError::Busy)        => Ok(-3),  // open file still holds the port
        Err(_)                                 => Ok(-1),  // NotFound / not mounted
    }
}

/// Resolve a `path` against `base` (current CWD). Handles `.`, `..`,
/// absolute path override, and trailing-slash normalization.
pub fn resolve_cwd(base: &str, path: &str) -> alloc::string::String {
    let mut out: Vec<&str> = Vec::new();
    let combined = if path.starts_with('/') {
        alloc::string::String::from(path)
    } else {
        let mut s = alloc::string::String::from(base);
        if !s.ends_with('/') { s.push('/'); }
        s.push_str(path);
        s
    };
    for seg in combined.split('/') {
        match seg {
            "" | "." => {}
            ".." => { out.pop(); }
            s => out.push(s),
        }
    }
    let mut result = alloc::string::String::from("/");
    result.push_str(&out.join("/"));
    if result.len() > 1 && result.ends_with('/') {
        result.pop();
    }
    result
}

/// ruos_exec_pipeline(buf_ptr, buf_len, exit_code_ptr) -> errno.
/// `buf` is the serialized stage list (see plan). Runs all stages concurrently
/// joined by pipes; writes the last stage's exit code at `exit_code_ptr`.
pub fn ruos_exec_pipeline(
    caller: Caller<'_, RuntimeState>,
    buf_ptr: i32,
    buf_len: i32,
    exit_code_ptr: i32,
) -> Result<i32, Error> {
    let blob = match crate::wasm::host::mem::guest_read(&caller, buf_ptr, buf_len) {
        Ok(b) => b,
        Err(e) => return Ok(e),
    };
    let raw_stages = match decode_pipeline(&blob) {
        Some(s) if !s.is_empty() => s,
        _ => return Ok(22), // EINVAL: malformed/empty
    };
    if raw_stages.len() > crate::wasm::pipeline::PIPE_MAX_STAGES {
        return Ok(7); // E2BIG: pipeline too long
    }
    let cwd = caller.data().cwd.clone();
    // Canonicalize each stage path and capability-check against the task's grant.
    let mut stages = Vec::with_capacity(raw_stages.len());
    for (stage_path, stage_argv) in raw_stages {
        let abs = resolve_cwd(&cwd, &stage_path);
        if !caller.data().grants(&abs) { return Ok(76); } // ENOTCAPABLE
        stages.push((abs, stage_argv));
    }
    // Inherit the calling shell's terminal (PTY) so the pipeline's
    // terminal-facing FDs reach the right console (e.g. the SSH PTY), not
    // the default /dev/pts/0. Falls back to 0 if fd 1 isn't a PTY.
    let (term_pts, term_src) = match caller.data().fds.get(1).and_then(|s| s.as_ref()) {
        Some(crate::wasm::state::FdEntry::Vfs(kfd)) => {
            match crate::vfs::fd::pts_index(*kfd) {
                Some(i) => (i, "vfs-pts"),
                None    => (0, "vfs-non-pts"),
            }
        }
        Some(_) => (0, "non-vfs"),
        None    => (0, "fd1-none"),
    };
    crate::binfo!("pipe", "exec_pipeline stages={} term_pts={} ({})",
                  stages.len(), term_pts, term_src);
    Err(Error::host(SuspendReason::ExecPipeline {
        stages,
        cwd,
        term_pts,
        exit_code_ptr: exit_code_ptr as u32,
    }))
}

/// Decode the pipeline blob. Returns Vec<(path, argv)>.
fn decode_pipeline(blob: &[u8]) -> Option<Vec<(String, Vec<Vec<u8>>)>> {
    let rd_u32 = |b: &[u8], o: usize| -> Option<u32> {
        if o + 4 > b.len() { return None; }
        Some(u32::from_le_bytes([b[o], b[o+1], b[o+2], b[o+3]]))
    };
    let mut o = 0usize;
    let n = rd_u32(blob, o)? as usize; o += 4;
    let mut stages = Vec::with_capacity(n);
    for _ in 0..n {
        let plen = rd_u32(blob, o)? as usize; o += 4;
        if o + plen > blob.len() { return None; }
        let path = core::str::from_utf8(&blob[o..o+plen]).ok()?.to_string(); o += plen;
        let argc = rd_u32(blob, o)? as usize; o += 4;
        let mut argv = Vec::with_capacity(argc);
        for _ in 0..argc {
            let alen = rd_u32(blob, o)? as usize; o += 4;
            if o + alen > blob.len() { return None; }
            argv.push(blob[o..o+alen].to_vec()); o += alen;
        }
        stages.push((path, argv));
    }
    Some(stages)
}

fn decode_argv(blob: &[u8]) -> Option<Vec<Vec<u8>>> {
    if blob.len() < 4 { return None; }
    let count = u32::from_le_bytes([blob[0], blob[1], blob[2], blob[3]]) as usize;
    let mut out = Vec::with_capacity(count);
    let table_start = 4usize;
    let table_end = table_start.checked_add(count.checked_mul(8)?)?;
    if blob.len() < table_end { return None; }
    for i in 0..count {
        let off = table_start + i * 8;
        let offset = u32::from_le_bytes([blob[off], blob[off+1], blob[off+2], blob[off+3]]) as usize;
        let length = u32::from_le_bytes([blob[off+4], blob[off+5], blob[off+6], blob[off+7]]) as usize;
        let end = offset.checked_add(length)?;
        if blob.len() < end { return None; }
        out.push(blob[offset..end].to_vec());
    }
    Some(out)
}

pub fn ruos_readdir(
    caller: Caller<'_, RuntimeState>,
    path_ptr: i32,
    path_len: i32,
    buf_ptr: i32,
    buf_len: i32,
    nread_ptr: i32,
) -> Result<i32, Error> {
    let path_buf = match crate::wasm::host::mem::guest_read(&caller, path_ptr, path_len) {
        Ok(b) => b,
        Err(e) => return Ok(e),
    };
    let path_str = core::str::from_utf8(&path_buf)
        .map_err(|_| Error::i32_exit(-1))?;
    let path = resolve_cwd(&caller.data().cwd, path_str);
    if !caller.data().grants(&path) { return Ok(76); } // ENOTCAPABLE
    Err(Error::host(SuspendReason::ReadDir {
        path,
        buf_ptr: buf_ptr as u32,
        buf_len: buf_len as usize,
        nread_ptr: nread_ptr as u32,
    }))
}

/// ruos_pci_list(buf_ptr, buf_len, used_ptr) -> errno.
/// Writes pre-formatted text (one device per line) to the caller buffer:
///   "BB:DD.F  VVVV:DDDD  CC SS PP  <class name>\n"
/// Returns 0 OK with the byte count at `used_ptr`. On buffer too small,
/// returns 8 (ENOBUFS) and still sets `used_ptr` to the required size.
pub fn ruos_pci_list(
    mut caller: Caller<'_, RuntimeState>,
    buf_ptr: i32,
    buf_len: i32,
    used_ptr: i32,
) -> Result<i32, Error> {
    let mut text = String::new();
    for d in crate::pci::devices() {
        let _ = writeln!(
            text,
            "{:02x}:{:02x}.{}  {:04x}:{:04x}  {:02x} {:02x} {:02x}  {}",
            d.address.bus(),
            d.address.device(),
            d.address.function(),
            d.vendor_id,
            d.device_id,
            d.class,
            d.subclass,
            d.prog_if,
            pci_class_name(d.class, d.subclass, d.prog_if),
        );
    }
    let bytes = text.as_bytes();
    let need = bytes.len() as u32;
    if let Err(e) = crate::wasm::host::mem::guest_write_u32(&mut caller, used_ptr, need) {
        return Ok(e);
    }
    if (buf_len as usize) < bytes.len() {
        return Ok(8); // ENOBUFS
    }
    if let Err(e) = crate::wasm::host::mem::guest_write(&mut caller, buf_ptr, bytes) {
        return Ok(e);
    }
    Ok(0)
}

fn pci_class_name(class: u8, sub: u8, prog_if: u8) -> &'static str {
    match (class, sub, prog_if) {
        (0x01, 0x06, _   ) => "SATA controller",
        (0x01, 0x08, _   ) => "NVMe controller",
        (0x01, _,    _   ) => "Mass storage controller",
        (0x02, 0x00, _   ) => "Ethernet controller",
        (0x02, _,    _   ) => "Network controller",
        (0x03, 0x00, _   ) => "VGA controller",
        (0x03, _,    _   ) => "Display controller",
        (0x04, _,    _   ) => "Multimedia controller",
        (0x06, 0x00, _   ) => "Host bridge",
        (0x06, 0x01, _   ) => "ISA bridge",
        (0x06, 0x04, _   ) => "PCI bridge",
        (0x06, _,    _   ) => "Bridge",
        (0x0C, 0x03, 0x30) => "xHCI USB controller",
        (0x0C, 0x03, 0x20) => "EHCI USB controller",
        (0x0C, 0x03, _   ) => "USB controller",
        (0x0C, _,    _   ) => "Serial bus controller",
        _                   => "Unclassified",
    }
}

/// ruos_usb_list(buf_ptr, buf_len, used_ptr) -> errno. Mirrors `ruos_pci_list`:
/// writes one enumerated USB device per line to the caller buffer:
///   "Bus BB Dev SS  Port P  Tier T  ID vvvv:pppp  <speed>  <kind>\n"
/// `Bus` = xHCI controller index, `Dev` = per-controller slot id. Returns 0 OK
/// with the byte count at `used_ptr`; on buffer too small returns 8 (ENOBUFS)
/// and still sets `used_ptr` to the required size.
pub fn ruos_usb_list(
    mut caller: Caller<'_, RuntimeState>,
    buf_ptr: i32,
    buf_len: i32,
    used_ptr: i32,
) -> Result<i32, Error> {
    let mut text = String::new();
    for d in crate::usb::registry::usb_list() {
        let _ = writeln!(
            text,
            "Bus {:02} Dev {:02}  Port {}  Tier {}  ID {:04x}:{:04x}  {:<5}  {}",
            d.ctrl,
            d.slot,
            d.root_port,
            d.tier,
            d.vid,
            d.pid,
            usb_speed_name(d.speed),
            d.kind,
        );
    }
    let bytes = text.as_bytes();
    let need = bytes.len() as u32;
    if let Err(e) = crate::wasm::host::mem::guest_write_u32(&mut caller, used_ptr, need) {
        return Ok(e);
    }
    if (buf_len as usize) < bytes.len() {
        return Ok(8); // ENOBUFS
    }
    if let Err(e) = crate::wasm::host::mem::guest_write(&mut caller, buf_ptr, bytes) {
        return Ok(e);
    }
    Ok(0)
}

/// xHCI PSI / hub-status speed code → human name (1=Full, 2=Low, 3=High, 4=Super).
fn usb_speed_name(speed: u8) -> &'static str {
    match speed {
        1 => "Full",
        2 => "Low",
        3 => "High",
        4 => "Super",
        _ => "?",
    }
}

/// ruos_sata_list(buf_ptr, buf_cap) -> i32.
/// Writes pre-formatted text (one SATA disk per line) into the caller buffer:
///   "<idx>\t<model>\t<N> MiB\n"
/// where `model` is the debug-formatted (`{:?}`, quoted) IDENTIFY string. The
/// `disks` tool strips the quotes when rendering its table.
///
/// Returns the number of bytes written, 0 if there are no SATA disks, or -1 if
/// the text does not fit in `buf_cap`. Mirrors `ruos_pci_list`'s guest-write
/// (same `crate::wasm::host::mem` boundary) but returns the length directly
/// rather than an errno + used-ptr.
fn ruos_sata_list(
    mut caller: Caller<'_, RuntimeState>,
    buf_ptr: i32,
    buf_cap: i32,
) -> Result<i32, Error> {
    let mut s = String::new();
    for idx in crate::ahci::sata_ports() {
        // Prefer the cache: a port brought up at boot (e.g. the one mounted at
        // /mnt) already has its (model, sectors) recorded. Only a NEVER-SEEN
        // port — which therefore can't be mounted — is brought up here, and that
        // first bringup caches it too. This guarantees `disks` never re-brings-
        // up (and so never corrupts) a live /mnt port.
        let info = match crate::ahci::disk_info(idx) {
            Some(i) => i,                                      // cached → NO bringup
            None => match crate::ahci::acquire_port(idx) {     // never seen → safe to bring up
                Some(p) => (p.model.clone(), p.sectors),
                None => continue,
            },
        };
        let _ = write!(s, "{}\t{:?}\t{} MiB\n", idx, info.0, info.1 / 2048);
    }
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return Ok(0); // no SATA disks
    }
    if buf_cap < 0 || bytes.len() > buf_cap as usize {
        return Ok(-1); // listing too large for the caller buffer
    }
    if let Err(e) = crate::wasm::host::mem::guest_write(&mut caller, buf_ptr, bytes) {
        return Ok(e);
    }
    Ok(bytes.len() as i32)
}

/// ruos_net_iface(buf_ptr, buf_len, used_ptr) -> errno.
/// Pre-formatted output:
///   "lo    127.0.0.1/8\n"
///   "eth0  10.0.2.15/24 mac=52:54:00:12:34:56 gw=10.0.2.2\n"
pub fn ruos_net_iface(
    mut caller: Caller<'_, RuntimeState>,
    buf_ptr: i32,
    buf_len: i32,
    used_ptr: i32,
) -> Result<i32, Error> {
    use smoltcp::wire::{IpAddress, IpCidr};
    let mut text = String::new();

    let mut g = crate::net::NET.lock();
    if let Some(net) = g.as_mut() {
        // Loopback
        for cidr in net.iface_lo.ip_addrs() {
            if let IpAddress::Ipv4(a) = cidr.address() {
                let _ = writeln!(text, "lo    {}/{}", a, cidr.prefix_len());
            }
        }
        // Ethernet (at most one of virtio xor hardware nic active).
        let (iface_opt, mac_opt) = if let (Some(iface), Some(dev)) =
            (net.iface_net.as_mut(), net.dev_net.as_ref())
        {
            (Some(iface), Some(dev.mac()))
        } else if let (Some(iface), Some(dev)) =
            (net.iface_nic.as_mut(), net.dev_nic.as_ref())
        {
            (Some(iface), Some(dev.mac()))
        } else {
            (None, None)
        };
        if let (Some(iface), Some(mac)) = (iface_opt, mac_opt) {
            let mac_str = alloc::format!(
                "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
            );
            let ip_part = iface
                .ip_addrs()
                .iter()
                .find_map(|c| match c {
                    IpCidr::Ipv4(c4) => Some(alloc::format!("{}/{}", c4.address(), c4.prefix_len())),
                    _ => None,
                })
                .unwrap_or_else(|| "0.0.0.0/0".to_string());
            // Default gateway: smoltcp Routes only exposes a mutable `update`
            // closure for iteration, so we borrow mutably and read inside.
            let mut gw_addr: Option<IpAddress> = None;
            iface.routes_mut().update(|v| {
                for r in v.iter() {
                    if r.cidr.prefix_len() == 0 {
                        gw_addr = Some(r.via_router);
                    }
                }
            });
            let gw = gw_addr.map(|a| alloc::format!(" gw={}", a)).unwrap_or_default();
            let _ = writeln!(text, "eth0  {} mac={}{}", ip_part, mac_str, gw);
        }
    }
    drop(g);

    let bytes = text.as_bytes();
    let need = bytes.len() as u32;
    if let Err(e) = crate::wasm::host::mem::guest_write_u32(&mut caller, used_ptr, need) {
        return Ok(e);
    }
    if (buf_len as usize) < bytes.len() {
        return Ok(8); // ENOBUFS
    }
    if let Err(e) = crate::wasm::host::mem::guest_write(&mut caller, buf_ptr, bytes) {
        return Ok(e);
    }
    Ok(0)
}

/// ruos_poweroff() -> never returns. Kernel halts the system.
pub fn ruos_poweroff(_caller: Caller<'_, RuntimeState>) -> Result<(), Error> {
    crate::kprintln!("ruos: poweroff requested by wasm");
    crate::power::poweroff();
}

/// ruos_reboot() -> never returns. Kernel reboots the system.
pub fn ruos_reboot(_caller: Caller<'_, RuntimeState>) -> Result<(), Error> {
    crate::kprintln!("ruos: reboot requested by wasm");
    crate::power::reboot();
}

/// ruos_net_set_static(ip0..3: i32, prefix: i32, gw0..3: i32, gw_present: i32)
///   → errno. Sets the active Ethernet interface to a static address +
///   default route. `gw_present=0` skips the gateway. Replaces any DHCP-bound
///   address. Returns 0 on success, errno otherwise (8 = no iface, 22 invalid).
#[allow(clippy::too_many_arguments)]
pub fn ruos_net_set_static(
    _caller: Caller<'_, RuntimeState>,
    ip0: i32, ip1: i32, ip2: i32, ip3: i32,
    prefix: i32,
    gw0: i32, gw1: i32, gw2: i32, gw3: i32,
    gw_present: i32,
) -> Result<i32, Error> {
    use smoltcp::wire::{IpAddress, IpCidr, Ipv4Address, Ipv4Cidr};
    if prefix < 0 || prefix > 32 { return Ok(22); } // EINVAL
    let addr = Ipv4Address::new(ip0 as u8, ip1 as u8, ip2 as u8, ip3 as u8);
    let cidr = Ipv4Cidr::new(addr, prefix as u8);
    let gw = if gw_present != 0 {
        Some(Ipv4Address::new(gw0 as u8, gw1 as u8, gw2 as u8, gw3 as u8))
    } else { None };

    let mut g = crate::net::NET.lock();
    let net = match g.as_mut() { Some(n) => n, None => return Ok(8) };
    // Apply to whichever Ethernet iface exists.
    let iface_opt = net.iface_net.as_mut().or_else(|| net.iface_nic.as_mut());
    let iface = match iface_opt { Some(i) => i, None => return Ok(8) };
    iface.update_ip_addrs(|a| {
        a.clear();
        a.push(IpCidr::Ipv4(cidr)).unwrap();
    });
    let _ = iface.routes_mut().remove_default_ipv4_route();
    if let Some(g) = gw {
        let _ = iface.routes_mut().add_default_ipv4_route(g);
    }
    // Cancel DHCP renew loop — operator override wins.
    if let Some(h) = net.dhcp.take() {
        net.net_sockets.remove(h);
    }
    crate::binfo!("net", "static ip={} gw={:?}", cidr, gw);
    Ok(0)
}

/// ruos_ping(ip0..3, timeout_ms, latency_ms_ptr) -> errno.
/// Sends one ICMP echo request, waits up to `timeout_ms` for a matching reply,
/// writes round-trip ms at `latency_ms_ptr` on success. Returns 110 on
/// timeout, other errno values on early failures (no iface = 8).
pub fn ruos_ping(
    _caller: Caller<'_, RuntimeState>,
    ip0: i32, ip1: i32, ip2: i32, ip3: i32,
    timeout_ms: i32,
    latency_ms_ptr: i32,
) -> Result<i32, Error> {
    use crate::wasm::suspend::SuspendReason;
    let target = smoltcp::wire::Ipv4Address::new(ip0 as u8, ip1 as u8, ip2 as u8, ip3 as u8);
    let ms = if timeout_ms <= 0 { 1000 } else { timeout_ms as u64 };
    // timer ticks @ 100 Hz = 10 ms each.
    let timeout_ticks = (ms + 9) / 10;
    Err(Error::host(SuspendReason::Ping {
        target,
        timeout_ticks,
        latency_ms_ptr: latency_ms_ptr as u32,
    }))
}

/// ruos_time_get(year_ptr, month_ptr, day_ptr, hour_ptr, min_ptr, sec_ptr,
///                epoch_ptr) -> errno. All fields are written through the
/// wasm-memory pointers; epoch_ptr receives a u64 unix seconds value.
#[allow(clippy::too_many_arguments)]
pub fn ruos_time_get(
    mut caller: Caller<'_, RuntimeState>,
    year_ptr: i32, month_ptr: i32, day_ptr: i32,
    hour_ptr: i32, min_ptr: i32, sec_ptr: i32,
    epoch_ptr: i32,
) -> Result<i32, Error> {
    let t = crate::rtc::now();
    let epoch = crate::rtc::to_unix_epoch(&t);
    macro_rules! wt {
        ($ptr:expr, $bytes:expr) => {
            if let Err(e) = crate::wasm::host::mem::guest_write(&mut caller, $ptr, $bytes) {
                return Ok(e);
            }
        };
    }
    wt!(year_ptr,  &t.year.to_le_bytes());
    wt!(month_ptr, &[t.month]);
    wt!(day_ptr,   &[t.day]);
    wt!(hour_ptr,  &[t.hour]);
    wt!(min_ptr,   &[t.minute]);
    wt!(sec_ptr,   &[t.second]);
    wt!(epoch_ptr, &epoch.to_le_bytes());
    Ok(0)
}

/// ruos_tcp_dial(ip0..3, port, fd_out_ptr) -> errno.
/// Allocate a TCP socket, inject it as a new wasm FD (written at fd_out_ptr),
/// then trap with SuspendReason::SockConnect so the fiber awaits Established
/// before returning to wasm. After success the caller can fd_read/fd_write on
/// the returned FD; close it with fd_close.
///
/// Local port: 49152 + (idx % 16384) — ephemeral range, deterministic per slot.
#[allow(clippy::too_many_arguments)]
pub fn ruos_tcp_dial(
    mut caller: Caller<'_, RuntimeState>,
    ip0: i32, ip1: i32, ip2: i32, ip3: i32,
    port: i32,
    fd_out_ptr: i32,
) -> Result<i32, Error> {
    use smoltcp::wire::{IpAddress, IpEndpoint};
    use crate::wasm::state::FdEntry;
    use crate::wasm::suspend::SuspendReason;

    if port <= 0 || port > 0xFFFF { return Ok(22); }

    // Check both caps before touching the kernel socket pool — avoids leaking a
    // socket allocation when the fd table is already full (MAX_FDS exhausted).
    {
        let fds = &caller.data().fds;
        let socket_count = fds.iter().filter(|s| matches!(s, Some(FdEntry::Socket(_)))).count();
        if socket_count >= crate::wasm::state::MAX_SOCKETS { return Ok(33); } // EMFILE (33)
        // Determine whether a free fd slot exists before we alloc a kernel socket.
        let fd_available = fds.iter().any(|s| s.is_none())
            || fds.len() < crate::wasm::state::MAX_FDS;
        if !fd_available { return Ok(33); } // EMFILE (33) — fd table full
    }

    let idx = crate::net::sockets::POOL.alloc_tcp();
    let handle = match crate::net::sockets::POOL.handle(idx) {
        Some(h) => h,
        None    => return Ok(8),
    };
    let remote = IpEndpoint::new(
        IpAddress::v4(ip0 as u8, ip1 as u8, ip2 as u8, ip3 as u8),
        port as u16,
    );
    let local_port: u16 = 49152u16.wrapping_add((idx as u16) & 0x3FFF);

    // Allocate a wasm-side FD pointing at this socket (bounded by MAX_FDS).
    // The fd-availability check above guarantees this cannot fail.
    let fd = {
        let fds = &mut caller.data_mut().fds;
        // Scan for a None slot first; else extend up to MAX_FDS.
        let mut slot = None;
        for (i, s) in fds.iter().enumerate() {
            if s.is_none() { slot = Some(i); break; }
        }
        match slot {
            Some(i) => { fds[i] = Some(FdEntry::Socket(idx)); i as i32 }
            None if fds.len() < crate::wasm::state::MAX_FDS => {
                fds.push(Some(FdEntry::Socket(idx)));
                (fds.len() - 1) as i32
            }
            // Should be unreachable given the pre-check above.
            None => return Ok(33), // EMFILE (33) — fd table full
        }
    };
    // Persist the FD to wasm memory for the caller.
    if let Err(e) = crate::wasm::host::mem::guest_write(&mut caller, fd_out_ptr, &fd.to_le_bytes()) {
        return Ok(e);
    }

    // Trap into SockConnect; the fiber driver awaits Established and resumes.
    Err(Error::host(SuspendReason::SockConnect { handle, remote, local_port }))
}

/// ruos_mkdisk(esp_mib) -> status. Author a fresh ruos disk (GPT + FAT32 ESP
/// with /EFI/BOOT + FAT32 data partition) on the FIRST populated SATA port.
/// **Destructive** — wipes the target disk.
///
/// Synchronous: brings up the first PI port via the boot-time HBA
/// (`ahci::acquire_port`) WITHOUT a second HBA reset, then calls `disk::author`.
/// The reset-free path means a live `/mnt` on another port is not orphaned.
///
/// Returns 0 on success; negative on failure: -1 no SATA port, -2 author error.
/// `esp_mib <= 0` defaults to 64 MiB; values above 4096 are clamped down.
pub fn ruos_mkdisk(_caller: Caller<'_, RuntimeState>, esp_mib: i32) -> Result<i32, Error> {
    // Acquire the first populated SATA port via the boot-time HBA — NO second
    // HBA reset, so a live /mnt on another port is not orphaned.
    let idx = match crate::ahci::sata_ports().first().copied() {
        Some(i) => i,
        None => {
            crate::bwarn!("mkdisk", "no populated SATA port");
            return Ok(-1);
        }
    };
    let mut port = match crate::ahci::acquire_port(idx) {
        Some(p) => p,
        None => {
            crate::bwarn!("mkdisk", "port {} acquire failed", idx);
            return Ok(-1);
        }
    };

    // Clamp/validate the ESP size: default 64 MiB, cap at 4096 MiB.
    let esp_mib: u32 = if esp_mib <= 0 {
        64
    } else if esp_mib > 4096 {
        4096
    } else {
        esp_mib as u32
    };

    match crate::disk::author(&mut port, esp_mib) {
        Ok(layout) => {
            // Commit the disk's write-back cache so the GPT + FAT32 partitions are
            // durable across a VM reset / power-off (WRITE DMA EXT alone leaves
            // them in cache on VBox/real HW).
            if port.flush().is_err() {
                crate::bwarn!("mkdisk", "cache flush failed (writes may not be durable)");
            }
            crate::binfo!(
                "mkdisk",
                "ok esp_lba={} esp_sec={} data_lba={} data_sec={}",
                layout.esp.first_lba, layout.esp.sectors,
                layout.data.first_lba, layout.data.sectors,
            );
            Ok(0)
        }
        Err(e) => {
            crate::bwarn!("mkdisk", "author failed: {:?}", e);
            Ok(-2)
        }
    }
}

/// ruos_mkboot(esp_mib) -> status. Author a fresh ruos disk (GPT + FAT32 ESP +
/// FAT32 data) on the FIRST populated SATA port, THEN write the boot tree:
/// bootstrap (kernel + init chain + shell + network/SSH service + the slim
/// limine.conf) onto the ESP so the SSD boots standalone (UEFI → BOOTX64.EFI →
/// limine.conf → kernel), and the command-line tools onto the data partition
/// (mount at /mnt/bin). **Destructive** — wipes the disk.
///
/// Synchronous: same reset-free port acquisition as `ruos_mkdisk`
/// (`ahci::acquire_port`), then `disk::author` followed by
/// `disk::copy_boot_payload`, which makes both partition borrows internally.
///
/// Returns 0 on success; negative on failure: -1 no SATA port, -2 author/copy
/// error. `esp_mib <= 0` defaults to 64 MiB; values above 4096 are clamped down.
pub fn ruos_mkboot(_caller: Caller<'_, RuntimeState>, esp_mib: i32) -> Result<i32, Error> {
    // Acquire the first populated SATA port via the boot-time HBA — NO second
    // HBA reset, so a live /mnt on another port is not orphaned.
    let idx = match crate::ahci::sata_ports().first().copied() {
        Some(i) => i,
        None => {
            crate::bwarn!("mkboot", "no populated SATA port");
            return Ok(-1);
        }
    };
    let mut port = match crate::ahci::acquire_port(idx) {
        Some(p) => p,
        None => {
            crate::bwarn!("mkboot", "port {} acquire failed", idx);
            return Ok(-1);
        }
    };

    // Clamp/validate the ESP size: default 64 MiB, cap at 4096 MiB.
    let esp_mib: u32 = if esp_mib <= 0 {
        64
    } else if esp_mib > 4096 {
        4096
    } else {
        esp_mib as u32
    };

    // Lay down the GPT + both FAT32 partitions + /EFI/BOOT.
    let layout = match crate::disk::author(&mut port, esp_mib) {
        Ok(l) => l,
        Err(e) => {
            crate::bwarn!("mkboot", "author failed: {:?}", e);
            return Ok(-2);
        }
    };

    // Write the boot tree: bootstrap to the ESP, tools to the data partition.
    // copy_boot_payload makes both partition borrows internally (sequentially).
    if crate::disk::copy_boot_payload(&mut port, &layout).is_err() {
        crate::bwarn!("mkboot", "copy_boot_payload failed");
        return Ok(-2);
    }

    // Commit the disk's write-back cache so the boot tree (ESP + data partition)
    // is durable across a VM reset / power-off before we report success.
    if port.flush().is_err() {
        crate::bwarn!("mkboot", "cache flush failed (writes may not be durable)");
    }

    crate::binfo!(
        "mkboot", "ok esp_lba={} data_lba={}",
        layout.esp.first_lba, layout.data.first_lba,
    );
    Ok(0)
}

/// ruos_install(esp_mib, target) -> status. The user-facing "install ruos to the
/// SSD" command. Two modes selected by `target`:
///
/// LIST mode (`target < 0`): no-op sentinel kept for compatibility. Read-only —
/// never authors, never needs the /mnt guard. Returns -10 ("list shown"). Disk
/// enumeration now lives in the `disks` tool (host fn `ruos_sata_list`); bare
/// `install` just points the operator at `disks` instead of logging here.
///
/// INSTALL mode (`target >= 0`): author a fresh ruos disk (GPT + FAT32 ESP +
/// FAT32 data) on SATA port `target` AND write the full boot tree onto its ESP —
/// the same work as `ruos_mkboot`, but GUARDED so it can never wipe the running
/// system's own data disk.
///
/// GUARD: refuses (returns -3, non-destructive) when a filesystem is mounted at
/// `/mnt`. A live `/mnt` means we booted off an installed/data SATA disk, and
/// authoring would orphan it; the operator must boot the installer medium (which
/// leaves `/mnt` unmounted) to install onto a target disk.
///
/// Returns 0 on success; negative otherwise: -10 list shown (target<0),
/// -3 /mnt mounted (guard tripped), -11 no SATA disk at `target`,
/// -1 port not ready, -2 author/copy error. `esp_mib <= 0` defaults to 64 MiB;
/// values above 4096 are clamped down.
fn ruos_install(_c: Caller<'_, RuntimeState>, esp_mib: i32, target: i32) -> Result<i32, Error> {
    let ports = crate::ahci::sata_ports();
    // --- LIST mode (target < 0): no-op sentinel, wipe nothing ---
    // Disk enumeration now lives in the `disks` tool (via ruos_sata_list); this
    // path no longer logs the per-disk lines `install` used to emit.
    if target < 0 {
        return Ok(-10); // sentinel: "list shown"
    }
    // --- INSTALL mode (target >= 0) ---
    // GUARD: never wipe the running system's data disk.
    if crate::vfs::is_mounted("/mnt") {
        crate::bwarn!("install", "refusing: /mnt is mounted -- boot the installer medium to install");
        return Ok(-3);
    }
    let idx = target as usize;
    if !ports.contains(&idx) {
        crate::bwarn!("install", "no SATA disk at index {} (run `install` with no argument to list)", target);
        return Ok(-11);
    }
    let esp = if esp_mib <= 0 { 64 } else if esp_mib > 4096 { 4096 } else { esp_mib } as u32;
    let mut port = match crate::ahci::acquire_port(idx) { Some(p) => p, None => return Ok(-1) };
    crate::binfo!("install", "target: port {} model={:?} sectors={} ({} MiB) -- WIPING",
        idx, port.model, port.sectors, port.sectors / 2048);
    let layout = match crate::disk::author(&mut port, esp) { Ok(l) => l, Err(_) => return Ok(-2) };
    if crate::disk::copy_boot_payload(&mut port, &layout).is_err() { return Ok(-2); }
    // Commit the disk's write-back cache so the full install (GPT + ESP + data
    // partition, ~22 MB) is durable across a VM reset / power-off. WRITE DMA EXT
    // alone leaves it in cache on VBox/real HW -> "install ok" but disk reverts.
    if port.flush().is_err() {
        crate::bwarn!("install", "cache flush failed (writes may not be durable)");
    }
    crate::binfo!("install", "ok -- ruos installed to port {}, reboot from the SSD", idx);
    Ok(0)
}

/// ruos_net_dhcp_renew() → errno. Restart DHCP client (if currently static).
pub fn ruos_net_dhcp_renew(_caller: Caller<'_, RuntimeState>) -> Result<i32, Error> {
    use smoltcp::socket::dhcpv4;
    let mut g = crate::net::NET.lock();
    let net = match g.as_mut() { Some(n) => n, None => return Ok(8) };
    if net.iface_net.is_none() && net.iface_nic.is_none() { return Ok(8); }
    if net.dhcp.is_none() {
        net.dhcp = Some(net.net_sockets.add(dhcpv4::Socket::new()));
        crate::binfo!("net", "dhcp renew requested");
    }
    Ok(0)
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker
        .func_wrap("ruos", "exec", ruos_exec)?
        .func_wrap("ruos", "readdir", ruos_readdir)?
        .func_wrap("ruos", "chdir", ruos_chdir)?
        .func_wrap("ruos", "umount", ruos_umount)?
        .func_wrap("ruos", "poweroff", ruos_poweroff)?
        .func_wrap("ruos", "reboot", ruos_reboot)?
        .func_wrap("ruos", "pci_list", ruos_pci_list)?
        .func_wrap("ruos", "usb_list", ruos_usb_list)?
        .func_wrap("ruos", "sata_list", ruos_sata_list)?
        .func_wrap("ruos", "net_iface", ruos_net_iface)?
        .func_wrap("ruos", "net_set_static", ruos_net_set_static)?
        .func_wrap("ruos", "net_dhcp_renew", ruos_net_dhcp_renew)?
        .func_wrap("ruos", "tcp_dial", ruos_tcp_dial)?
        .func_wrap("ruos", "time_get", ruos_time_get)?
        .func_wrap("ruos", "ping", ruos_ping)?
        .func_wrap("ruos", "mkdisk", ruos_mkdisk)?
        .func_wrap("ruos", "mkboot", ruos_mkboot)?
        .func_wrap("ruos", "install", ruos_install)?
        .func_wrap("ruos", "exec_pipeline", ruos_exec_pipeline)?;
    Ok(())
}
