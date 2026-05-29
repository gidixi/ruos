//! Custom (non-WASIX) host fns: ruos_exec + ruos_readdir + introspection.

use wasmi::{Caller, Linker, Error};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;
use crate::wasm::state::RuntimeState;
use crate::wasm::host::lifecycle::wasm_memory;
use crate::wasm::suspend::SuspendReason;

pub fn ruos_exec(
    caller: Caller<'_, RuntimeState>,
    path_ptr: i32,
    path_len: i32,
    argv_ptr: i32,
    argv_len: i32,
    exit_code_ptr: i32,
) -> Result<i32, Error> {
    let mem = wasm_memory(&caller)?;
    let mut path_buf = alloc::vec![0u8; path_len as usize];
    mem.read(&caller, path_ptr as usize, &mut path_buf)
        .map_err(|_| Error::i32_exit(-1))?;
    let path = core::str::from_utf8(&path_buf)
        .map_err(|_| Error::i32_exit(-1))?
        .to_string();
    let mut argv_blob = alloc::vec![0u8; argv_len as usize];
    mem.read(&caller, argv_ptr as usize, &mut argv_blob)
        .map_err(|_| Error::i32_exit(-1))?;
    let argv = decode_argv(&argv_blob).unwrap_or_default();
    // Child inherits parent's CWD — POSIX semantics.
    let cwd = caller.data().cwd.clone();
    Err(Error::host(SuspendReason::Exec {
        path,
        argv,
        cwd,
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
    let mem = wasm_memory(&caller)?;
    let mut path_buf = alloc::vec![0u8; path_len as usize];
    mem.read(&caller, path_ptr as usize, &mut path_buf)
        .map_err(|_| Error::i32_exit(-1))?;
    let path = core::str::from_utf8(&path_buf)
        .map_err(|_| Error::i32_exit(-1))?;
    let new_cwd = resolve_cwd(&caller.data().cwd, path);
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
    let mem = wasm_memory(&caller)?;
    let mut path_buf = alloc::vec![0u8; path_len as usize];
    mem.read(&caller, path_ptr as usize, &mut path_buf)
        .map_err(|_| Error::i32_exit(-1))?;
    let path_str = core::str::from_utf8(&path_buf)
        .map_err(|_| Error::i32_exit(-1))?;
    let path = resolve_cwd(&caller.data().cwd, path_str);
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
    let mem = wasm_memory(&caller)?;
    let need = bytes.len() as u32;
    mem.write(&mut caller, used_ptr as usize, &need.to_le_bytes())
        .map_err(|e| Error::new(alloc::format!("pci_list used write: {}", e)))?;
    if (buf_len as usize) < bytes.len() {
        return Ok(8); // ENOBUFS
    }
    mem.write(&mut caller, buf_ptr as usize, bytes)
        .map_err(|e| Error::new(alloc::format!("pci_list buf write: {}", e)))?;
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
    let mem = wasm_memory(&caller)?;
    let need = bytes.len() as u32;
    mem.write(&mut caller, used_ptr as usize, &need.to_le_bytes())
        .map_err(|e| Error::new(alloc::format!("net_iface used write: {}", e)))?;
    if (buf_len as usize) < bytes.len() {
        return Ok(8); // ENOBUFS
    }
    mem.write(&mut caller, buf_ptr as usize, bytes)
        .map_err(|e| Error::new(alloc::format!("net_iface buf write: {}", e)))?;
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
        .func_wrap("ruos", "poweroff", ruos_poweroff)?
        .func_wrap("ruos", "reboot", ruos_reboot)?
        .func_wrap("ruos", "pci_list", ruos_pci_list)?
        .func_wrap("ruos", "net_iface", ruos_net_iface)?
        .func_wrap("ruos", "net_set_static", ruos_net_set_static)?
        .func_wrap("ruos", "net_dhcp_renew", ruos_net_dhcp_renew)?;
    Ok(())
}
