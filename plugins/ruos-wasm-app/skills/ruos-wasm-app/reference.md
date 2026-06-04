# ruos host-function & WASI reference

For a tool that needs a kernel capability `std` can't express. Read this instead
of grepping the kernel each time. Authoritative source: the `.func_wrap("ruos", …)`
registrations in `kernel/src/wasm/host/` (mostly `proc.rs`) — grep there for a
fn's exact current signature; this file documents the patterns + the common ones.

## How a host fn works

ruos exposes kernel calls as wasm imports in the `ruos` module:
```rust
#[link(wasm_import_module = "ruos")]
extern "C" { fn sata_list(ptr: i32, cap: i32) -> i32; }
// ...
let r = unsafe { sata_list(buf.as_mut_ptr() as i32, buf.len() as i32) };
```
Kernel side, each is `fn ruos_<name>(caller: Caller<'_, RuntimeState>, args…) ->
Result<i32, Error>` registered with `.func_wrap("ruos", "<name>", ruos_<name>)?`.
Args are scalars (`i32`/`i64`); **strings and buffers cross the boundary as
`(ptr, len)` pairs**, read/written via `crate::wasm::host::mem::{guest_read,
guest_write}` (which bounds-check against the guest's linear memory).

Two dominant conventions:

- **buffer-OUT** (info fns): the tool passes `(ptr, cap)` of a scratch buffer; the
  kernel formats text into it and returns the **byte length** (or `-1` too-small,
  `0` empty). The tool parses/prints. Example: `sata_list`, `proc_list`, `meminfo`,
  `uname`, `dmesg`, `pci_list`.
- **path/blob-IN** (action fns): the tool passes `(ptr, len)` of an input string;
  the kernel reads it and acts, returning `0` ok / negative errno. Example:
  `chdir`, `umount`, `exec`.

## Common host fns (verify exact arg counts in proc.rs before use)

```
// --- buffer-OUT (ptr, cap) -> bytes_written | -1 too-small | 0 empty ---
fn uname(ptr: i32, cap: i32) -> i32;        // os/version/arch string
fn meminfo(ptr: i32, cap: i32) -> i32;      // memory totals
fn cpuinfo(ptr: i32, cap: i32) -> i32;      // per-core info
fn cpustat(ptr: i32, cap: i32) -> i32;      // per-core CPU% (cooperative TSC acct)
fn uptime(ptr: i32, cap: i32) -> i32;
fn dmesg(ptr: i32, cap: i32) -> i32;        // the kernel boot log
fn proc_list(ptr: i32, cap: i32) -> i32;    // running wasm tasks
fn proc_stat(pid: i32, ptr: i32, cap: i32) -> i32;
fn pci_list(ptr: i32, cap: i32) -> i32;
fn sata_list(ptr: i32, cap: i32) -> i32;    // <idx>\t<{:?}-quoted model>\t<size>
fn net_iface(ptr: i32, cap: i32) -> i32;
fn service_list(ptr: i32, cap: i32) -> i32;

// --- action / scalar ---
fn chdir(ptr: i32, len: i32) -> i32;        // path in; 0 ok / errno
fn umount(ptr: i32, len: i32) -> i32;       // path in; 0 ok / -1 not mounted / -2 cannot / -3 busy
fn exec(argv_ptr: i32, argv_len: i32, …) -> i32;   // run a /bin/*.wasm (used by the shell)
fn install(esp_mib: i32, target: i32) -> i32;      // target<0 list, >=0 install to that SATA disk
fn mkdisk(esp_mib: i32) -> i32;             // author first SATA disk (diag)
fn mkboot(esp_mib: i32) -> i32;             // author + copy boot tree (diag)
fn proc_kill(pid: i32, sig: i32) -> i32;
fn time_get(/* clock id */) -> i64;         // a clock
fn poweroff() -> i32;  fn reboot() -> i32;
fn isatty(fd: i32) -> i32;
fn tcgetattr(fd: i32, ptr: i32, cap: i32) -> i32;   // termios blob (56 bytes)
fn tcsetattr(fd: i32, ptr: i32, len: i32) -> i32;
fn poll_stdin(timeout_ms: i32) -> i32;      // for interactive/TUI tools (rtop uses it)
fn smp_bench(/* … */) -> i64;               // offload a CPU job to the AP pool
```
The full set (grep `\.func_wrap("ruos"` in `kernel/src/wasm`): `chdir cpuinfo
cpustat dmesg exec exec_pipeline install isatty meminfo mkboot mkdisk
net_dhcp_renew net_iface net_set_static pci_list ping poll_stdin poweroff
proc_kill proc_list proc_stat readdir reboot sata_list service_list service_start
service_status smp_bench tcgetattr tcp_dial tcsetattr time_get umount uname
uptime`. **Always confirm the arg list against the actual `ruos_<name>` fn** —
signatures evolve (e.g. `install` gained a `target` arg).

## WASI (`std`) — what works without any host fn

- **Files:** `std::fs` over the VFS — `read`, `write`, `File`, `read_dir`
  (`fd_readdir` is exported), `metadata`, `create_dir`. Paths: `/` (tmpfs), `/tmp`,
  `/dev/{console,null,zero}`, `/bin`, `/etc`, `/mnt` (FAT data partition, when
  mounted), `/mnt/bin` (installed-SSD tools).
- **Args/env:** `std::env::args`, `std::env::var`.
- **Stdout/stderr:** `println!`, `eprintln!`, `print!`, `std::io::{stdin,stdout}`.
- **Exit:** `std::process::exit(code)`.
- **Pipes:** the shell wires `cmd1 | cmd2` via the kernel; your tool just reads
  stdin / writes stdout.
- **Crates:** any `no-default-features`/wasip1-friendly crate (e.g. `ratatui` 0.29
  for TUIs — see `rtop`). Avoid crates needing threads, sockets-via-std, or
  unsupported syscalls.

Not available via std: raw sockets (use `tcp_dial`/the net host fns), threads
(single-core cooperative), `SystemTime::now` semantics beyond `time_get`.

## Adding a NEW host fn (kernel change — only if `std` + the existing fns can't do it)

1. In `kernel/src/wasm/host/proc.rs` (or a sibling), write `fn ruos_<name>(caller:
   Caller<'_, RuntimeState>, args…) -> Result<i32, Error>`. Mirror an existing one:
   `ruos_chdir` for a path-IN fn, `ruos_pci_list`/`ruos_sata_list` for a buffer-OUT
   fn. Use `crate::wasm::host::mem::guest_read(&caller, ptr, len)` /
   `guest_write(&mut caller, ptr, bytes)` for the boundary (they bounds-check —
   never index guest memory directly).
2. Register it where the others are: `.func_wrap("ruos", "<name>", ruos_<name>)?`.
3. Keep it panic-safe: map every path to `Ok(<i32 code>)` — a panic in a host fn
   aborts the kernel. No `unwrap`/`expect` on guest-derived input.
4. Declare the matching `extern "C"` import in your tool.
5. `make iso` rebuilds both kernel + tool.

## Examples in-tree to copy

- Plain std: `user/echo`, `user/cat`, `user/uname`.
- Buffer-OUT host fn: `user/disks` (`sata_list`), `user/lspci` (`pci_list`),
  `user/ps` (`proc_list`).
- Action host fn: `user/install`, `user/umount`.
- Interactive/TUI: `user/rtop` (`ratatui` + `poll_stdin` + termios).
