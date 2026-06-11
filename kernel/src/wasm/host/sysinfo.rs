//! Custom `ruos` host fns that expose kernel state to userspace
//! (uname, uptime, meminfo, cpuinfo, dmesg, ps, kill). Synchronous —
//! none of these block, so no SuspendReason needed.

use wasmi::{Caller, Linker, Error};
use crate::wasm::state::RuntimeState;

const KERNEL_NAME:   &str = "ruos";
const KERNEL_REL:    &str = "0.1.0";
const KERNEL_VER:    &str = "wasm-userland";
const KERNEL_MACH:   &str = "x86_64";
const KERNEL_NODE:   &str = "ruos";

pub(crate) fn write_bytes_and_len(
    mut caller: Caller<'_, RuntimeState>,
    buf_ptr: i32,
    buf_len: i32,
    used_ptr: i32,
    data: &[u8],
) -> Result<i32, Error> {
    let n = data.len().min(buf_len.max(0) as usize);
    if let Err(e) = crate::wasm::host::mem::guest_write(&mut caller, buf_ptr, &data[..n]) {
        return Ok(e);
    }
    if let Err(e) = crate::wasm::host::mem::guest_write_u32(&mut caller, used_ptr, n as u32) {
        return Ok(e);
    }
    Ok(0)
}

/// ruos_uname(buf_ptr, buf_len, used_ptr) -> errno
/// Writes "name\0node\0release\0version\0machine\0" — NUL-separated, no
/// trailing NUL. Compact, easy to parse from userspace.
pub fn ruos_uname(
    caller: Caller<'_, RuntimeState>,
    buf_ptr: i32,
    buf_len: i32,
    used_ptr: i32,
) -> Result<i32, Error> {
    let mut s = alloc::vec::Vec::new();
    s.extend_from_slice(KERNEL_NAME.as_bytes()); s.push(0);
    s.extend_from_slice(KERNEL_NODE.as_bytes()); s.push(0);
    s.extend_from_slice(KERNEL_REL.as_bytes());  s.push(0);
    s.extend_from_slice(KERNEL_VER.as_bytes());  s.push(0);
    s.extend_from_slice(KERNEL_MACH.as_bytes());
    write_bytes_and_len(caller, buf_ptr, buf_len, used_ptr, &s)
}

/// ruos_uptime() -> u64 centiseconds since boot. Timer fires at 100 Hz,
/// so the tick count is already in centiseconds — no scaling needed.
pub fn ruos_uptime(_: Caller<'_, RuntimeState>) -> Result<i64, Error> {
    Ok(crate::timer::ticks() as i64)
}

/// meminfo blob: 4 u64 little-endian —
///   heap_total_bytes, heap_used_bytes (0 if unavailable),
///   frames_total, frames_used.
/// Shared by the wasmi "ruos" host fn and the wt component `ruos:tui/host`.
pub fn meminfo_blob() -> [u8; 32] {
    let heap_total = crate::memory::HEAP_SIZE as u64;
    // talc 4.x does not expose a stable "bytes in use" API in our cfg, so
    // we leave heap_used as 0 — userspace `free` prints "?" for that
    // column. Patch when/if we wire up talc stats.
    let heap_used: u64 = 0;
    let frames = crate::memory::frame_counts();
    let mut out = [0u8; 32];
    out[0..8].copy_from_slice(&heap_total.to_le_bytes());
    out[8..16].copy_from_slice(&heap_used.to_le_bytes());
    out[16..24].copy_from_slice(&frames.total.to_le_bytes());
    out[24..32].copy_from_slice(&frames.used.to_le_bytes());
    out
}

/// ruos_meminfo(buf_ptr) -> errno
pub fn ruos_meminfo(
    mut caller: Caller<'_, RuntimeState>,
    buf_ptr: i32,
) -> Result<i32, Error> {
    let out = meminfo_blob();
    if let Err(e) = crate::wasm::host::mem::guest_write(&mut caller, buf_ptr, &out) {
        return Ok(e);
    }
    Ok(0)
}

/// Read CPUID vendor + brand string via raw asm. Returns
/// "vendor\0brand\0n_cpus_dec".
pub fn ruos_cpuinfo(
    caller: Caller<'_, RuntimeState>,
    buf_ptr: i32,
    buf_len: i32,
    used_ptr: i32,
) -> Result<i32, Error> {
    use core::arch::x86_64::__cpuid;
    let mut s = alloc::vec::Vec::new();
    // Vendor: CPUID EAX=0 -> EBX,EDX,ECX (12 bytes ASCII).
    let v = unsafe { __cpuid(0) };
    s.extend_from_slice(&v.ebx.to_le_bytes());
    s.extend_from_slice(&v.edx.to_le_bytes());
    s.extend_from_slice(&v.ecx.to_le_bytes());
    s.push(0);
    // Brand: CPUID EAX=0x80000002..0x80000004 -> 48 bytes ASCII (if
    // supported). Probe via max-extended-fn (EAX=0x80000000).
    let max_ext = unsafe { __cpuid(0x80000000) }.eax;
    if max_ext >= 0x80000004 {
        for leaf in [0x80000002u32, 0x80000003, 0x80000004] {
            let r = unsafe { __cpuid(leaf) };
            s.extend_from_slice(&r.eax.to_le_bytes());
            s.extend_from_slice(&r.ebx.to_le_bytes());
            s.extend_from_slice(&r.ecx.to_le_bytes());
            s.extend_from_slice(&r.edx.to_le_bytes());
        }
        // Trim trailing NULs that the brand string left in place.
        while s.last() == Some(&0) { s.pop(); }
    } else {
        s.extend_from_slice(b"(no brand)");
    }
    s.push(0);
    // n_cpus = BSP (1) + online APs (Fase 1 SMP bring-up).
    let n_cpus = 1 + crate::cpu::cpus_online();
    let mut numbuf = alloc::string::String::new();
    {
        use core::fmt::Write as _;
        let _ = write!(numbuf, "{}", n_cpus);
    }
    s.extend_from_slice(numbuf.as_bytes());
    write_bytes_and_len(caller, buf_ptr, buf_len, used_ptr, &s)
}

/// ruos_dmesg(buf_ptr, buf_len, used_ptr) -> errno
pub fn ruos_dmesg(
    mut caller: Caller<'_, RuntimeState>,
    buf_ptr: i32,
    buf_len: i32,
    used_ptr: i32,
) -> Result<i32, Error> {
    let mut tmp = alloc::vec![0u8; buf_len.max(0) as usize];
    let n = crate::klog::read(&mut tmp);
    if let Err(e) = crate::wasm::host::mem::guest_write(&mut caller, buf_ptr, &tmp[..n]) {
        return Ok(e);
    }
    if let Err(e) = crate::wasm::host::mem::guest_write_u32(&mut caller, used_ptr, n as u32) {
        return Ok(e);
    }
    Ok(0)
}

/// ruos_proc_list(buf_ptr, buf_len, used_ptr) -> errno
///
/// Layout written at buf_ptr:
///   u32 count
///   for each: u32 pid, u64 start_tick, u16 name_len, u16 pad, name_bytes
/// Truncates silently if buf_len too small (caller resizes and retries).
pub fn ruos_proc_list(
    mut caller: Caller<'_, RuntimeState>,
    buf_ptr: i32,
    buf_len: i32,
    used_ptr: i32,
) -> Result<i32, Error> {
    let procs = crate::proc::list();
    let mut blob = alloc::vec::Vec::new();
    blob.extend_from_slice(&(procs.len() as u32).to_le_bytes());
    for p in &procs {
        blob.extend_from_slice(&p.pid.to_le_bytes());
        blob.extend_from_slice(&p.start_tick.to_le_bytes());
        let name = p.name.as_bytes();
        let nl = (name.len() as u16).to_le_bytes();
        blob.extend_from_slice(&nl);
        blob.extend_from_slice(&[0u8, 0u8]); // pad
        blob.extend_from_slice(name);
    }
    let n = blob.len().min(buf_len.max(0) as usize);
    if let Err(e) = crate::wasm::host::mem::guest_write(&mut caller, buf_ptr, &blob[..n]) {
        return Ok(e);
    }
    if let Err(e) = crate::wasm::host::mem::guest_write_u32(&mut caller, used_ptr, blob.len() as u32) {
        return Ok(e);
    }
    Ok(0)
}

/// ruos_proc_kill(pid) -> 0 if signaled, 3 (ESRCH) if pid unknown.
pub fn ruos_proc_kill(_: Caller<'_, RuntimeState>, pid: i32) -> Result<i32, Error> {
    if crate::proc::request_kill(pid as u32) { Ok(0) } else { Ok(3) }
}

/// proc_stat blob (shared wasmi/wt):
///   u32 count
///   for each: u32 pid, u64 start_tick, u64 cpu_tsc, u64 mem_bytes,
///             u16 name_len, u16 pad, name_bytes
pub fn proc_stat_blob() -> alloc::vec::Vec<u8> {
    let procs = crate::proc::list();
    let mut blob = alloc::vec::Vec::new();
    blob.extend_from_slice(&(procs.len() as u32).to_le_bytes());
    for p in &procs {
        blob.extend_from_slice(&p.pid.to_le_bytes());
        blob.extend_from_slice(&p.start_tick.to_le_bytes());
        blob.extend_from_slice(&p.cpu_tsc.to_le_bytes());
        blob.extend_from_slice(&p.mem_bytes.to_le_bytes());
        let name = p.name.as_bytes();
        blob.extend_from_slice(&(name.len() as u16).to_le_bytes());
        blob.extend_from_slice(&[0u8, 0u8]); // pad
        blob.extend_from_slice(name);
    }
    blob
}

/// ruos_proc_stat(buf_ptr, buf_len, used_ptr) -> errno
///
/// Like proc_list but each row carries cpu_tsc + mem_bytes:
///   u32 count
///   for each: u32 pid, u64 start_tick, u64 cpu_tsc, u64 mem_bytes,
///             u16 name_len, u16 pad, name_bytes
/// Writes the *full* required length to used_ptr; truncates the bytes
/// written to buf_len so the caller can resize and retry.
pub fn ruos_proc_stat(
    mut caller: Caller<'_, RuntimeState>,
    buf_ptr: i32,
    buf_len: i32,
    used_ptr: i32,
) -> Result<i32, Error> {
    let blob = proc_stat_blob();
    let n = blob.len().min(buf_len.max(0) as usize);
    if let Err(e) = crate::wasm::host::mem::guest_write(&mut caller, buf_ptr, &blob[..n]) {
        return Ok(e);
    }
    if let Err(e) = crate::wasm::host::mem::guest_write_u32(&mut caller, used_ptr, blob.len() as u32) {
        return Ok(e);
    }
    Ok(0)
}

/// ruos_cpustat(buf_ptr, buf_len) -> errno
///
/// Little-endian blob:
///   u32 ncores
///   u64 tsc_per_ms
///   for each core: u64 busy_tsc, u64 idle_tsc
/// ncores = 1 (BSP) + online APs. Returns 0, or 8 (ERANGE) if buf too small.
/// cpustat blob (shared wasmi/wt): u32 ncores, u64 tsc_per_ms, then per
/// core u64 busy_tsc, u64 idle_tsc. ncores = 1 (BSP) + online APs.
pub fn cpustat_blob() -> alloc::vec::Vec<u8> {
    let ncores = 1 + crate::cpu::cpus_online() as usize;
    let mut blob = alloc::vec::Vec::new();
    blob.extend_from_slice(&(ncores as u32).to_le_bytes());
    blob.extend_from_slice(&crate::boot::clock::tsc_per_ms().to_le_bytes());
    for cpu in 0..ncores {
        let (busy, idle) = crate::sched::cpustat::read(cpu);
        blob.extend_from_slice(&busy.to_le_bytes());
        blob.extend_from_slice(&idle.to_le_bytes());
    }
    blob
}

pub fn ruos_cpustat(
    mut caller: Caller<'_, RuntimeState>,
    buf_ptr: i32,
    buf_len: i32,
) -> Result<i32, Error> {
    let blob = cpustat_blob();
    if (buf_len.max(0) as usize) < blob.len() {
        return Ok(8); // ERANGE
    }
    if let Err(e) = crate::wasm::host::mem::guest_write(&mut caller, buf_ptr, &blob) {
        return Ok(e);
    }
    Ok(0)
}

pub fn link(linker: &mut Linker<RuntimeState>) -> Result<(), Error> {
    linker
        .func_wrap("ruos", "uname",      ruos_uname)?
        .func_wrap("ruos", "uptime",     ruos_uptime)?
        .func_wrap("ruos", "meminfo",    ruos_meminfo)?
        .func_wrap("ruos", "cpuinfo",    ruos_cpuinfo)?
        .func_wrap("ruos", "dmesg",      ruos_dmesg)?
        .func_wrap("ruos", "proc_list",  ruos_proc_list)?
        .func_wrap("ruos", "proc_kill",  ruos_proc_kill)?
        .func_wrap("ruos", "cpustat",    ruos_cpustat)?
        .func_wrap("ruos", "proc_stat",  ruos_proc_stat)?;
    Ok(())
}
