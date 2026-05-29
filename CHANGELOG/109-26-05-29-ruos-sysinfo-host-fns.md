# 109 — ruos sysinfo host fns

**Data:** 2026-05-29

## Cosa
Nuovo modulo `kernel/src/wasm/host/sysinfo.rs` che espone:
- `ruos_uname(buf, len, used)` → "name\0node\0release\0version\0machine"
- `ruos_uptime() -> i64` (centiseconds da boot, da timer::ticks)
- `ruos_meminfo(buf)` → 4 u64 LE: heap_total, heap_used (0 finché non
  esponiamo talc stats), frames_total, frames_used
- `ruos_cpuinfo(buf, len, used)` → "vendor\0brand\0n_cpus" via CPUID
- `ruos_dmesg(buf, len, used)` → contenuti del klog ring buffer
- `ruos_proc_list(buf, len, used)` → header u32 count + entries
  (pid u32, start_tick u64, name_len u16, pad u16, name bytes)
- `ruos_proc_kill(pid) -> errno` (0 se segnalato, 3 ESRCH se pid ignoto)

Tutte sincrone (no SuspendReason — non bloccano).

## Perché
Backend per i .wasm tool `uname`, `uptime`, `free`, `df`, `lscpu`,
`dmesg`, `ps`, `kill`, `pkill`. Senza queste host fns, userspace non
può vedere/manipolare lo stato kernel.

## File toccati
- kernel/src/wasm/host/sysinfo.rs (nuovo)
- kernel/src/wasm/host/mod.rs
