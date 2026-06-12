# Module `sys` — telemetry

Live kernel telemetry (CPU / processes / memory / uptime) for GUI apps like the
System Monitor. Runtime: **Wasmtime AOT** (`.cwasm`).
Source: `kernel/src/wasm/wt/sys.rs` (`func_wrap("sys", …)`).
Guest declarations: `ruos-desktop/crates/ruos-window/src/lib.rs` (`mod sys`).

Blob fields are little-endian. Return: `0` OK, `8` ERANGE (buffer too small),
`21` EFAULT. Same layouts as the wasmi `ruos` module that `rtop` uses.

**Last reviewed:** 2026-06-11 (5 functions; `events_poll` added — kernel event
bus reader for compositor windows, registered in `wm.rs`).

```rust
#[link(wasm_import_module = "sys")]
extern "C" { /* signatures below */ }
```

---

### `cpustat(ptr: *mut u8, len: u32) -> i32`
Write a per-core busy/idle snapshot at `ptr`:

```
u32 ncores
u64 tsc_per_ms
ncores × { u64 busy_tsc, u64 idle_tsc }
```

`ncores = 1 (BSP) + online APs`. Divide deltas by `tsc_per_ms` for ms; busy/(busy+idle)
for utilization.

### `proc_stat(ptr: *mut u8, len: u32, used: *mut u32) -> i32`
Write the process table at `ptr` and the **required** byte count at `used` (so the
caller can resize + retry on `8`):

```
u32 count
count × {
  u32 pid
  u64 start_tick          // 100 Hz
  u64 cpu_tsc             // cumulative TSC charged to the proc
  u64 mem_bytes           // last-observed wasm linear-memory size
  u16 name_len
  u16 pad
  u8  name[name_len]      // UTF-8
}
```

Kernel daemons appear with bracketed names (e.g. `[watchdog]`) and `cpu_tsc=mem=0`.

### `meminfo(ptr: *mut u8) -> i32`
Write **32 bytes** at `ptr`:

```
u64 heap_total      // talc heap size
u64 heap_used       // 0 if unavailable
u64 frames_total    // physical frames
u64 frames_used
```

### `uptime() -> i64`
Centiseconds since boot (100 Hz tick).

### `events_poll(buf_ptr) -> i32`
One kernel-event-bus record per call, from THIS window's private cursor.
Returns `1` (64-byte LE record written: seq u64 | kind u16 | severity u8 | pad[1]
@11 | ts_ticks u32 | payload 4×u32 | name 32B NUL-padded) or `0` (nothing new).
On reader overflow a synthetic `SUBSCRIBER_OVERFLOW` (kind 0x0001, payload =
lost lo/hi) is delivered first. Kinds/payloads: see
`docs/superpowers/specs/2026-06-11-kernel-event-bus-design.md` §2.
