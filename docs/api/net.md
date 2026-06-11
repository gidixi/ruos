# `net` — TCP + DNS for GUI window apps (Wasmtime AOT)

Non-blocking, poll-based networking for `.cwasm` window apps. The window path is
fully synchronous (the compositor calls `frame()` inline; no fiber, no epoch), so
**no fn here ever blocks** — a blocking fetch would freeze the whole desktop.
Call from the frame loop and treat every fn as one poll step.

**Last reviewed:** 2026-06-11 (7 functions).

```rust
#[link(wasm_import_module = "net")]
extern "C" {
    fn resolve_start(name_ptr: *const u8, name_len: u32) -> i32;
    fn resolve_poll(req: i32, ip_out_ptr: *mut u8) -> i32;
    fn dial(ip0: u32, ip1: u32, ip2: u32, ip3: u32, port: u32) -> i64;
    fn state(sock: i32) -> i32;
    fn read(sock: i32, ptr: *mut u8, len: u32) -> i32;
    fn write(sock: i32, ptr: *const u8, len: u32) -> i32;
    fn close(sock: i32);
}
```

## DNS

### `resolve_start(name_ptr, name_len) -> i32`
Begin resolving a hostname (≤253 bytes, UTF-8). Returns a request handle
(`>= 0`) or `-1` (invalid name / resolver busy — ≤8 in flight). The resolve runs
on the BSP executor (`net::dns::resolve`, UDP against the DHCP-provided server).

### `resolve_poll(req, ip_out_ptr) -> i32`
Poll a pending resolve. `1` = done — the first A record (4 bytes, network order)
is written at `ip_out_ptr` and the handle is freed. `0` = still pending (poll
again next frame). `-1` = failed / unknown handle (handle freed).

## TCP

### `dial(ip0..3, port) -> i64`
Allocate an Ethernet-side TCP socket and **initiate** a connect (SYN goes out on
the next `net_poll_task` tick). Returns a socket handle (`>= 0`) immediately —
the connection is NOT established yet; poll `state` until `1`. Errors: `-22`
EINVAL (bad port), `-8` (pool), `-111` (connect could not start). Local port is
ephemeral (49152 + slot).

### `state(sock) -> i32`
`0` = handshake in progress · `1` = Established · `2` = closed (refused, reset,
or finished) · `-1` = invalid handle.

### `read(sock, ptr, len) -> i32`
`> 0` bytes copied into guest memory (≤64 KiB per call — the socket RX buffer
size) · `0` = peer closed the read side · `-1` = no data right now
(would-block; try next frame) · `-2` = invalid args/handle.

### `write(sock, ptr, len) -> i32`
`> 0` bytes accepted into the TX buffer (may be < len; advance and retry) ·
`0` = closed for write · `-1` = TX buffer full (would-block) · `-2` = invalid.

### `close(sock)`
Graceful close + release. The kernel reclaims the socket (and frees the slot
for reuse) once the FIN handshake completes. The handle is **invalid** after
this call — do not pass it to `read`/`write`/`state` again.

## Typical fetch (frame-loop state machine)

```text
Resolve(req)      resolve_poll == 1 → Dial
Dial(sock)        state == 1        → send request via write (loop on -1)
Sent              read > 0          → accumulate; read == 0 → done
```

Sockets ride the same kernel smoltcp pool as the wasmi CLI path
(`kernel/src/net/sockets.rs`, driven by `net_poll_task` on the BSP);
registration in `kernel/src/wasm/wt/net.rs`.
