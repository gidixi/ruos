//! `sys` host module for compositor (Wasmtime) windows: live CPU/memory/process
//! telemetry for the System Monitor app. Mirrors the wasmi `host/sysinfo.rs` blob
//! layouts so the guest parser is the same shape, but for the `wt` `Linker<T>`
//! (generic over the store type; reads global kernel state, writes guest memory
//! via the single audited `wt::mem` path). Errno-style returns: 0 ok, 8 ERANGE,
//! 21 EFAULT (guest write out of bounds).

use wasmtime::{Caller, Linker};
use alloc::vec::Vec;

/// Register `sys.{cpustat,proc_stat,meminfo,uptime}` on a window linker. Generic
/// over `T` (these fns never touch the store data — only global kernel state +
/// guest memory), so the same call works for `Linker<AppState>` and any other.
pub fn add_to_linker<T>(linker: &mut Linker<T>) -> wasmtime::Result<()> {
    // sys.cpustat(buf_ptr, buf_len) -> i32: u32 ncores, u64 tsc_per_ms, then
    // ncores × (u64 busy, u64 idle). 8 = ERANGE if the buffer is too small.
    linker.func_wrap("sys", "cpustat",
        |mut caller: Caller<'_, T>, buf_ptr: i32, buf_len: i32| -> i32 {
            let ncores = 1 + crate::cpu::cpus_online() as usize;
            let mut blob: Vec<u8> = Vec::new();
            blob.extend_from_slice(&(ncores as u32).to_le_bytes());
            blob.extend_from_slice(&crate::boot::clock::tsc_per_ms().to_le_bytes());
            for cpu in 0..ncores {
                let (busy, idle) = crate::sched::cpustat::read(cpu);
                blob.extend_from_slice(&busy.to_le_bytes());
                blob.extend_from_slice(&idle.to_le_bytes());
            }
            if (buf_len.max(0) as usize) < blob.len() { return 8; }
            if !crate::wasm::wt::mem::write(&mut caller, buf_ptr as u32, &blob) { return 21; }
            0
        })?;
    // sys.proc_stat(buf_ptr, buf_len, used_ptr) -> i32: u32 count then per-row
    // pid/start_tick/cpu_tsc/mem_bytes/name. Writes the FULL required length to
    // used_ptr; truncates the bytes to buf_len so the guest can size up + retry.
    linker.func_wrap("sys", "proc_stat",
        |mut caller: Caller<'_, T>, buf_ptr: i32, buf_len: i32, used_ptr: i32| -> i32 {
            let procs = crate::proc::list();
            let mut blob: Vec<u8> = Vec::new();
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
            let n = blob.len().min(buf_len.max(0) as usize);
            if !crate::wasm::wt::mem::write(&mut caller, buf_ptr as u32, &blob[..n]) { return 21; }
            if !crate::wasm::wt::mem::write_u32(&mut caller, used_ptr as u32, blob.len() as u32) { return 21; }
            0
        })?;
    // sys.meminfo(buf_ptr) -> i32: 4 × u64 (heap_total, heap_used, frames_total,
    // frames_used). heap_used = 0 (talc has no stable used-bytes API in our cfg).
    linker.func_wrap("sys", "meminfo",
        |mut caller: Caller<'_, T>, buf_ptr: i32| -> i32 {
            let heap_total = crate::memory::HEAP_SIZE as u64;
            let heap_used: u64 = 0;
            let frames = crate::memory::frame_counts();
            let mut out = [0u8; 32];
            out[0..8].copy_from_slice(&heap_total.to_le_bytes());
            out[8..16].copy_from_slice(&heap_used.to_le_bytes());
            out[16..24].copy_from_slice(&frames.total.to_le_bytes());
            out[24..32].copy_from_slice(&frames.used.to_le_bytes());
            if !crate::wasm::wt::mem::write(&mut caller, buf_ptr as u32, &out) { return 21; }
            0
        })?;
    // sys.uptime() -> i64: centiseconds since boot (100 Hz tick → already cs).
    linker.func_wrap("sys", "uptime",
        |_caller: Caller<'_, T>| -> i64 { crate::timer::ticks() as i64 })?;
    Ok(())
}
