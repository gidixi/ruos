# Module `ruos` — CLI tool host functions

Custom host functions for CLI tools. Runtime: **wasmi** interpreter (`.wasm`,
`wasm32-wasip1`). Imported alongside [`wasi`](wasi.md).
Sources: `kernel/src/wasm/host/proc.rs`, `sysinfo.rs` (+ tty/service/smp helpers).

```rust
#[link(wasm_import_module = "ruos")]
extern "C" { /* signatures below */ }
```

Convention: `*_ptr`/`*_len` are guest addresses; functions that fill a buffer take
`(buf, len, used_ptr)` and return `8` (ENOBUFS) with the required size at `used_ptr`
so the caller resizes + retries. Return `i32` = errno (`0` OK) unless noted.

**Last reviewed:** 2026-06-10.

---

## Process & filesystem  (`proc.rs`)

### `exec(path_ptr, path_len, argv_ptr, argv_len, exit_code_ptr) -> i32`
Run the binary at `path` with the serialized `argv` blob; the child inherits the
caller's cwd + PTY. Writes the child's exit code at `exit_code_ptr`. Suspends
(`SuspendReason::Exec`). `76` = ENOTCAPABLE (outside the capability grant).

### `exec_pipeline(buf_ptr, buf_len, exit_code_ptr) -> i32`
Run a serialized pipeline `a | b | …` (per-stage: path_len, path, argc, args). Writes
the LAST stage's exit code. `22` EINVAL, `7` E2BIG, `76` ENOTCAPABLE.

### `readdir(path_ptr, path_len, buf_ptr, buf_len, nread_ptr) -> i32`
Enumerate directory entries into `buf`; byte count at `nread_ptr`. Suspends
(`SuspendReason::ReadDir`). Resolves `path` against the kernel cwd (absolute or
relative). This is what `ls` uses (so `ls` honors cwd).

### `chdir(path_ptr, path_len) -> i32`
Change the kernel cwd (validated: must exist + be a directory). `44` ENOENT,
`54` ENOTDIR, `76` ENOTCAPABLE. The new cwd is injected into children as `PWD`.

### `umount(path_ptr, path_len) -> i32`
Unmount the filesystem at `path`. `-2` invalid (e.g. `/`), `-3` EBUSY, `-1` not
mounted, `0` OK.

## Hardware enumeration  (`proc.rs`)

Each returns pre-formatted text (one device per line).

### `pci_list(buf, len, used) -> i32`
`BB:DD.F  VVVV:DDDD  CC SS PP  <class name>\n`. `8` ENOBUFS.

### `usb_list(buf, len, used) -> i32`
`Bus BB Dev SS  Port P  Tier T  ID vvvv:pppp  <speed>  <kind>\n`. `8` ENOBUFS.

### `sata_list(buf, cap) -> i32`
`<idx>\t<model>\t<N> MiB\n`. Returns byte count, `0` no disks, `-1` buffer too small.

### `wifi_scan(buf, cap) -> i32`
Scan for WiFi networks via the RTL8188EU USB dongle. Lazily brings the chip up
(power-on + firmware + MAC/BB/RF init) on the FIRST call (~1-2 s), then runs a
passive 2.4 GHz scan. Fills `buf` with `<ssid>\t<channel>\t<security>\n` per AP.
Returns byte count, `0` no device / no APs in range, `-1` buffer too small. Used
by the `wifiscan` tool.

## System info  (`sysinfo.rs`)

### `uname(buf, len, used) -> i32`
`name\0node\0release\0version\0machine` (no trailing NUL).

### `uptime() -> i64`
Centiseconds since boot.

### `meminfo(buf) -> i32`
32 bytes LE: `u64 heap_total, heap_used, frames_total, frames_used`.

### `cpuinfo(buf, len, used) -> i32`
`vendor\0brand\0n_cpus` (CPUID + online core count).

### `dmesg(buf, len, used) -> i32`
Copy the kernel log ring; length at `used`.

### `cpustat(buf, len) -> i32`
Per-core busy/idle (same blob as [`sys.cpustat`](sys.md)). `8` ERANGE.

### `proc_list(buf, len, used) -> i32`
`u32 count`, then per proc `u32 pid, u64 start_tick, u16 name_len, u16 pad, name`.

### `proc_stat(buf, len, used) -> i32`
Like `proc_list` + `u64 cpu_tsc, u64 mem_bytes` per row (same as [`sys.proc_stat`](sys.md)).

### `proc_kill(pid) -> i32`
Cooperative kill (sets a flag the target checks at its next host call). `0` OK,
`3` ESRCH — **kernel daemons refuse** (protected).

## Networking  (`proc.rs`)

### `net_iface(buf, len, used) -> i32`
Interface list, e.g. `lo  127.0.0.1/8\n`, `eth0  10.0.2.15/24 mac=… gw=…\n`. `8` ENOBUFS.

### `wifi_connect(ssid_ptr, ssid_len, pass_ptr, pass_len, buf_ptr, buf_cap) -> i32`
Connect to a WPA2-PSK network on the RTL8188EU dongle: lazily brings the chip up,
scans for `ssid`, runs open-system authentication + WPA2 association (assoc-request
carries the WPA2-PSK / CCMP RSN IE), then — when `pass` is non-empty — the WPA2
4-way handshake (HMAC-SHA1 PTK/MIC, AES GTK unwrap) and installs the CCMP PTK+GTK
into the chip's HW key CAM. Writes a one-line status into `buf`
(`auth=<ok|rejected|no-response> assoc=<…> aid=<N> 4way=<ok|failed|skipped>`, or
`ssid not found`). Returns the byte count, `0` no device, `-1` bad args / buffer
too small. With an empty `pass` the handshake is skipped (`4way=skipped`). The
encrypted data path + DHCP (smoltcp over the Wi-Fi link) is SP-WIFI-5.

### `net_set_static(ip0, ip1, ip2, ip3, prefix, gw0, gw1, gw2, gw3, gw_present) -> i32`
Set a static IP on the active NIC. `0` OK, `8` no iface, `22` EINVAL.

### `net_dhcp_renew() -> i32`
Restart the DHCP client. `0` OK, `8` no iface.

### `ping(ip0, ip1, ip2, ip3, timeout_ms, latency_ms_ptr) -> i32`
ICMP echo (suspends, `SuspendReason::Ping`); writes RTT ms at `latency_ms_ptr`.
`0` OK, `110` timeout.

### `net_resolve(name_ptr, name_len, addrs_out_ptr, max_addrs, count_out_ptr) -> i32`
DNS lookup for a hostname (suspends, `SuspendReason::NetResolve`).
`name_ptr/len` is the utf-8 hostname. `addrs_out_ptr` is a buffer where up to `max_addrs` IPv4 addresses (4 bytes each) will be written. `count_out_ptr` receives the number of addresses actually written.
`0` OK, `44` ENOENT (not found / timeout / no servers), `28` EINVAL.

### `tcp_dial(ip0, ip1, ip2, ip3, port, fd_out_ptr) -> i32`
Open a TCP socket, inject it as a guest fd (written at `fd_out_ptr`), connect
(suspends). `22` invalid port, `8` no iface, `33` EMFILE. Then use WASI `fd_read`/
`fd_write` on the fd.

## Disk & boot  (`proc.rs`)  — ⚠ DESTRUCTIVE

### `mkdisk(esp_mib) -> i32`
Author a fresh GPT + FAT32 (ESP + data) on the first SATA disk. `0` OK, `-1` no
port, `-2` author error.

### `mkboot(esp_mib) -> i32`
`mkdisk` + write the full boot tree (ESP + `/mnt/bin`). Same error codes.

### `install(esp_mib, target) -> i32`
Install ruos to SATA `target` (guarded: refuses if `/mnt` is mounted). `target < 0`
= LIST mode (`-10` sentinel). INSTALL: `0` OK, `-3` guard, `-11` no disk, `-1` not
ready, `-2` author error.

## Time & power  (`proc.rs`)

### `time_get(year_ptr, month_ptr, day_ptr, hour_ptr, min_ptr, sec_ptr, epoch_ptr) -> i32`
Write RTC fields to the pointers; `epoch_ptr` gets `u64` unix seconds.

### `poweroff()` / `reboot()`
Halt / restart. Never return.

## Terminal control / services / SMP

| Function | Meaning |
|----------|---------|
| `tcgetattr(fd, termios_ptr) -> i32` | Read PTY termios. `25` ENOTTY. |
| `tcsetattr(fd, action, termios_ptr) -> i32` | Write PTY termios. `25` ENOTTY. |
| `isatty(fd) -> i32` | `1` if `fd` is a PTY, else `0`. |
| `poll_stdin(buf_ptr, timeout_ticks) -> i32` | Wait one stdin byte (suspends); `1` got it, `0` timeout, `-1` EOF. |
| `service_list(buf, len, used) -> i32` | TSV `name\tstatus\tpid\truns\tpath\n`. |
| `service_start(name_ptr, name_len) -> i32` | Start a service (async). `1` NotFound, `2` Already, `3` NotSupported, `99` Internal. |
| `service_status(name_ptr, name_len, buf, len, used) -> i32` | One service's TSV row. `1` NotFound. |
| `smp_bench(buf, len, used) -> i32` | Parallel-vs-sequential hash benchmark report. |

> Some entries in this last group are helper host fns added incrementally; verify
> the exact signature against the source file before relying on it, and refine this
> page (per the [maintenance rule](README.md)).
