//! ruos host-fn bindings + little-endian blob parsers for rtop.
//!
//! The host fns (`cpustat`, `proc_stat`, `meminfo`, `uptime`) are imported
//! from module "ruos". The parsers are pure and unit-tested on the host.

extern crate alloc;

#[derive(Debug, Clone, PartialEq)]
pub struct CoreStat { pub busy: u64, pub idle: u64 }

#[derive(Debug, Clone, PartialEq)]
pub struct CpuStat { pub tsc_per_ms: u64, pub cores: alloc::vec::Vec<CoreStat> }

#[derive(Debug, Clone, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub start_tick: u64,
    pub cpu_tsc: u64,
    pub mem_bytes: u64,
    pub name: alloc::string::String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MemInfo {
    pub heap_total: u64,
    pub heap_used: u64,
    pub frames_total: u64,
    pub frames_used: u64,
}

fn rd_u16(b: &[u8], o: usize) -> u16 { u16::from_le_bytes([b[o], b[o+1]]) }
fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o+1], b[o+2], b[o+3]])
}
fn rd_u64(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o+8]);
    u64::from_le_bytes(a)
}

/// Decode the cpustat blob. Returns None on a short/garbled buffer.
pub fn parse_cpustat(b: &[u8]) -> Option<CpuStat> {
    if b.len() < 12 { return None; }
    let ncores = rd_u32(b, 0) as usize;
    let tsc_per_ms = rd_u64(b, 4);
    let mut cores = alloc::vec::Vec::with_capacity(ncores);
    let mut o = 12;
    for _ in 0..ncores {
        if o + 16 > b.len() { return None; }
        cores.push(CoreStat { busy: rd_u64(b, o), idle: rd_u64(b, o + 8) });
        o += 16;
    }
    Some(CpuStat { tsc_per_ms, cores })
}

/// Decode the proc_stat blob (uses only the first `used` bytes).
pub fn parse_proc_stat(b: &[u8], used: usize) -> alloc::vec::Vec<Proc> {
    let mut out = alloc::vec::Vec::new();
    if used < 4 { return out; }
    let count = rd_u32(b, 0) as usize;
    let mut o = 4usize;
    for _ in 0..count {
        if o + 30 > used { break; } // 4+8+8+8+2 header = 30 then name
        let pid = rd_u32(b, o);
        let start_tick = rd_u64(b, o + 4);
        let cpu_tsc = rd_u64(b, o + 12);
        let mem_bytes = rd_u64(b, o + 20);
        let nl = rd_u16(b, o + 28) as usize;
        let name_start = o + 32; // +28 namelen, +2 pad
        if name_start + nl > used { break; }
        let name = alloc::string::String::from_utf8_lossy(
            &b[name_start..name_start + nl]).into_owned();
        out.push(Proc { pid, start_tick, cpu_tsc, mem_bytes, name });
        o = name_start + nl;
    }
    out
}

/// Decode the 32-byte meminfo blob.
pub fn parse_meminfo(b: &[u8]) -> MemInfo {
    if b.len() < 32 { return MemInfo::default(); }
    MemInfo {
        heap_total: rd_u64(b, 0),
        heap_used: rd_u64(b, 8),
        frames_total: rd_u64(b, 16),
        frames_used: rd_u64(b, 24),
    }
}

#[link(wasm_import_module = "ruos")]
extern "C" {
    fn cpustat(buf_ptr: u32, buf_len: u32) -> i32;
    fn proc_stat(buf_ptr: u32, buf_len: u32, used_ptr: u32) -> i32;
    fn meminfo(buf_ptr: u32) -> i32;
    fn uptime() -> i64;
    fn poll_stdin(buf_ptr: u32, timeout_ticks: i64) -> i32;
}

/// Wait up to `timeout_ticks` (100 Hz) for one stdin byte. Returns `(code,
/// byte)`: code 1 = `byte` is a keystroke, 0 = timeout (redraw), -1 = EOF.
#[cfg(target_arch = "wasm32")]
pub fn poll_key(timeout_ticks: i64) -> (i32, u8) {
    let mut b = [0u8; 1];
    let r = unsafe { poll_stdin(b.as_mut_ptr() as u32, timeout_ticks) };
    (r, b[0])
}

/// Max cores we size the cpustat buffer for (matches kernel MAX_CPUS).
pub const MAX_CORES: usize = 16;

/// One full system snapshot read from the kernel.
pub struct Snapshot {
    pub cpu: CpuStat,
    pub procs: alloc::vec::Vec<Proc>,
    pub mem: MemInfo,
    pub uptime_cs: u64,
}

/// Read a snapshot via the ruos host fns. Returns None if cpustat fails.
#[cfg(target_arch = "wasm32")]
pub fn read_snapshot() -> Option<Snapshot> {
    use alloc::vec;
    // cpustat
    let mut cbuf = vec![0u8; 4 + 8 + 16 * MAX_CORES];
    let rc = unsafe { cpustat(cbuf.as_mut_ptr() as u32, cbuf.len() as u32) };
    if rc != 0 { return None; }
    let cpu = parse_cpustat(&cbuf)?;
    // proc_stat (grow-and-retry once)
    let mut pbuf = vec![0u8; 8192];
    let mut used: u32 = 0;
    let _ = unsafe { proc_stat(pbuf.as_mut_ptr() as u32, pbuf.len() as u32, &mut used as *mut u32 as u32) };
    if used as usize > pbuf.len() {
        pbuf = vec![0u8; used as usize];
        let _ = unsafe { proc_stat(pbuf.as_mut_ptr() as u32, pbuf.len() as u32, &mut used as *mut u32 as u32) };
    }
    let procs = parse_proc_stat(&pbuf, (used as usize).min(pbuf.len()));
    // meminfo
    let mut mbuf = [0u8; 32];
    let _ = unsafe { meminfo(mbuf.as_mut_ptr() as u32) };
    let mem = parse_meminfo(&mbuf);
    let uptime_cs = unsafe { uptime() } as u64;
    Some(Snapshot { cpu, procs, mem, uptime_cs })
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec::Vec;

    fn push_u32(v: &mut Vec<u8>, x: u32) { v.extend_from_slice(&x.to_le_bytes()); }
    fn push_u64(v: &mut Vec<u8>, x: u64) { v.extend_from_slice(&x.to_le_bytes()); }
    fn push_u16(v: &mut Vec<u8>, x: u16) { v.extend_from_slice(&x.to_le_bytes()); }

    #[test]
    fn cpustat_two_cores() {
        let mut b = Vec::new();
        push_u32(&mut b, 2);
        push_u64(&mut b, 1000);
        push_u64(&mut b, 100); push_u64(&mut b, 900);  // core0
        push_u64(&mut b, 0);   push_u64(&mut b, 1000); // core1
        let s = parse_cpustat(&b).unwrap();
        assert_eq!(s.tsc_per_ms, 1000);
        assert_eq!(s.cores.len(), 2);
        assert_eq!(s.cores[0], CoreStat { busy: 100, idle: 900 });
        assert_eq!(s.cores[1], CoreStat { busy: 0, idle: 1000 });
    }

    #[test]
    fn cpustat_short_buffer_is_none() {
        assert!(parse_cpustat(&[0u8; 6]).is_none());
    }

    #[test]
    fn proc_stat_one_row() {
        let mut b = Vec::new();
        push_u32(&mut b, 1);          // count
        push_u32(&mut b, 7);          // pid
        push_u64(&mut b, 50);         // start_tick
        push_u64(&mut b, 12345);      // cpu_tsc
        push_u64(&mut b, 131072);     // mem_bytes
        let name = b"shell.wasm";
        push_u16(&mut b, name.len() as u16);
        push_u16(&mut b, 0);          // pad
        b.extend_from_slice(name);
        let used = b.len();
        let v = parse_proc_stat(&b, used);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0], Proc {
            pid: 7, start_tick: 50, cpu_tsc: 12345,
            mem_bytes: 131072, name: "shell.wasm".into(),
        });
    }

    #[test]
    fn meminfo_roundtrip() {
        let mut b = Vec::new();
        push_u64(&mut b, 4_194_304); push_u64(&mut b, 0);
        push_u64(&mut b, 1024); push_u64(&mut b, 80);
        let m = parse_meminfo(&b);
        assert_eq!(m.heap_total, 4_194_304);
        assert_eq!(m.frames_total, 1024);
        assert_eq!(m.frames_used, 80);
    }
}
