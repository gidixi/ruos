# Netconsole UDP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stream every kernel log line (INFO+) over UDP broadcast `255.255.255.255:6666` so a `nc -ul 6666` listener on the LAN sees ruos's logs without a serial cable or SSH session; gated behind `--features netconsole`.

**Architecture:** A 4th log sink. `emit()` only *enqueues* bytes into a dedicated ring (never touches the `NET` lock → no deadlock with the net poll task). `net::poll()` — which already holds `NET` — drains the ring into a UDP socket each tick and the ethernet `iface.poll` transmits it. On the first DHCP bind the whole `dmesg` (klog) ring is pushed in as a backlog so logs from T+0 are recovered.

**Tech Stack:** Rust `no_std`, smoltcp 0.11 `socket-udp`, kernel `klog` ring, compile-time cargo feature.

---

## Verification note

QEMU/VM broadcast does not reach the physical LAN → **no automated test**. Gates:
1. **Default build unchanged:** `make iso` (feature off) compiles; `make run-test` passes.
2. **Feature build:** `make iso CARGO_FEATURES=netconsole` compiles.
3. **Real HW:** `nc -ul 6666` on another LAN host shows the stream after DHCP bind.

All commands via WSL: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && <cmd>'`.

---

## File Structure

- **Create** `kernel/src/net/netconsole.rs` — the whole feature: `NcRing` byte queue, `enqueue`, `init`, `mark_bound`, `on_poll`. Entirely `#[cfg(feature = "netconsole")]`.
- **Modify** `kernel/Cargo.toml` — add `netconsole = ["smoltcp/socket-udp"]` feature.
- **Modify** `kernel/src/net/mod.rs` — cfg-gated `pub mod netconsole;` + 3 hooks (init, poll drain, bind flag).
- **Modify** `kernel/src/boot/log.rs` — cfg-gated `enqueue` call in `emit`.
- **Create** `CHANGELOG/355-26-06-08-netconsole-udp.md`.

---

## Task 1: Feature flag + netconsole module

**Files:**
- Modify: `kernel/Cargo.toml`
- Create: `kernel/src/net/netconsole.rs`
- Modify: `kernel/src/net/mod.rs` (module declaration only)

- [ ] **Step 1: Add the cargo feature**

In `kernel/Cargo.toml`, under `[features]`, add the `netconsole` line:

```toml
[features]
boot-checks = []
netconsole = ["smoltcp/socket-udp"]
```

(Enabling `smoltcp/socket-udp` only when `netconsole` is on keeps the default build's smoltcp feature set unchanged.)

- [ ] **Step 2: Create `kernel/src/net/netconsole.rs`**

```rust
//! Netconsole — UDP broadcast log sink (compile-time `--features netconsole`).
//!
//! `emit()` (boot/log.rs) calls `enqueue()` for every log line. `enqueue` ONLY
//! pushes into `NC_RING` — it never locks `NET`, because `emit()` itself runs
//! inside the net poll task (the NIC driver logs while holding `NET`); a
//! synchronous UDP send from `emit()` would re-lock `NET` and deadlock.
//!
//! `net::poll()` (which already holds `NET`) calls `on_poll()` once per tick to
//! drain `NC_RING` into a UDP socket bound on the Ethernet `SocketSet`; the
//! ethernet `iface.poll` right after transmits it as a broadcast datagram.
//!
//! Nothing in the drain/send path logs (`binfo!`/`bwarn!`) — that would re-enter
//! `enqueue` and feed back on itself.

use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use smoltcp::iface::{SocketHandle, SocketSet};
use smoltcp::socket::udp;
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};

/// UDP port for both the local bind and the broadcast destination.
const PORT: u16 = 6666;
/// Ring capacity — holds the 32 KiB klog backlog plus live burst headroom.
const RING_CAP: usize = 48 * 1024;
/// Max datagram payload per send (kept well under a 1500-byte MTU).
const CHUNK: usize = 512;
/// Max datagrams flushed per poll tick (≈4 KiB/tick @ 100 Hz = ~400 KiB/s).
const MAX_PER_TICK: usize = 8;

/// True once DHCP has bound an address; before that `enqueue` is a no-op
/// (pre-bind lines live in klog and are recovered by the backlog flush).
static BOUND: AtomicBool = AtomicBool::new(false);
/// Handle of the UDP socket inside the Ethernet `SocketSet`.
static NC_HANDLE: Mutex<Option<SocketHandle>> = Mutex::new(None);
/// The byte queue. Producer = `enqueue`; consumer = `on_poll`.
static NC_RING: Mutex<NcRing> = Mutex::new(NcRing::new());

/// Bounded byte ring. On overflow the oldest bytes are dropped.
struct NcRing {
    buf:  [u8; RING_CAP],
    tail: usize, // oldest byte
    len:  usize, // bytes currently queued
}

impl NcRing {
    const fn new() -> Self {
        Self { buf: [0; RING_CAP], tail: 0, len: 0 }
    }

    fn push(&mut self, bytes: &[u8]) {
        for &b in bytes {
            let head = (self.tail + self.len) % RING_CAP;
            self.buf[head] = b;
            if self.len == RING_CAP {
                // Full: overwrite oldest, advance tail.
                self.tail = (self.tail + 1) % RING_CAP;
            } else {
                self.len += 1;
            }
        }
    }

    /// Copy up to `out.len()` queued bytes (oldest first) without consuming.
    fn peek(&self, out: &mut [u8]) -> usize {
        let n = self.len.min(out.len());
        for i in 0..n {
            out[i] = self.buf[(self.tail + i) % RING_CAP];
        }
        n
    }

    /// Drop `n` oldest bytes.
    fn advance(&mut self, n: usize) {
        let n = n.min(self.len);
        self.tail = (self.tail + n) % RING_CAP;
        self.len -= n;
    }
}

/// Create + bind the UDP socket on the Ethernet socket set. Called from
/// `net::init` after `net_sockets` is built.
pub fn init(net_sockets: &mut SocketSet<'static>) {
    let rx_meta = alloc::vec![udp::PacketMetadata::EMPTY; 4];
    let rx_buf  = alloc::vec![0u8; 256];
    let tx_meta = alloc::vec![udp::PacketMetadata::EMPTY; 32];
    let tx_buf  = alloc::vec![0u8; 8192];
    let mut sock = udp::Socket::new(
        udp::PacketBuffer::new(rx_meta, rx_buf),
        udp::PacketBuffer::new(tx_meta, tx_buf),
    );
    if sock.bind(PORT).is_err() {
        return; // leave NC_HANDLE None → on_poll stays a no-op
    }
    let handle = net_sockets.add(sock);
    *NC_HANDLE.lock() = Some(handle);
}

/// Queue a log line. No-op until DHCP-bound. Never touches `NET`.
pub fn enqueue(bytes: &[u8]) {
    if !BOUND.load(Ordering::Relaxed) {
        return;
    }
    NC_RING.lock().push(bytes);
}

/// Mark the interface bound and push the klog backlog so logs from T+0 reach
/// the listener. Idempotent. Called from `net::poll` on the DHCP bind edge,
/// while `NET` is held.
pub fn mark_bound() {
    if BOUND.swap(true, Ordering::Relaxed) {
        return; // already bound
    }
    let mut backlog = alloc::vec![0u8; 32 * 1024];
    let n = crate::klog::read(&mut backlog);
    NC_RING.lock().push(&backlog[..n]);
}

/// Drain the ring into the UDP socket. Called from `net::poll` (holds `NET`)
/// BEFORE the ethernet `iface.poll`, so the same poll transmits. Must not log.
pub fn on_poll(net_sockets: &mut SocketSet<'static>) {
    if !BOUND.load(Ordering::Relaxed) {
        return;
    }
    let handle = match *NC_HANDLE.lock() {
        Some(h) => h,
        None => return,
    };
    let sock = net_sockets.get_mut::<udp::Socket>(handle);
    let ep = IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::BROADCAST), PORT);

    let mut ring = NC_RING.lock();
    let mut tmp = [0u8; CHUNK];
    for _ in 0..MAX_PER_TICK {
        let n = ring.peek(&mut tmp);
        if n == 0 {
            break;
        }
        // Prefer a datagram that ends on a line boundary for readable output.
        let cut = tmp[..n].iter().rposition(|&b| b == b'\n').map(|i| i + 1).unwrap_or(n);
        let cut = cut.max(1);
        match sock.send_slice(&tmp[..cut], ep) {
            Ok(()) => ring.advance(cut),
            Err(_) => break, // tx buffer full → retry next tick, keep bytes queued
        }
    }
}
```

- [ ] **Step 3: Declare the module in `net/mod.rs`**

In `kernel/src/net/mod.rs`, add the cfg-gated module declaration next to the other `pub mod` lines (after `pub mod loopback;`):

```rust
pub mod icmp;
pub mod loopback;
#[cfg(feature = "netconsole")]
pub mod netconsole;
pub mod nic;
pub mod sockets;
pub mod virtio;
```

- [ ] **Step 4: Build feature-off (default)**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | tail -20'`
Expected: compiles, ISO assembled. `netconsole.rs` is NOT compiled (cfg off); zero new warnings.

- [ ] **Step 5: Build feature-on**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso CARGO_FEATURES=netconsole 2>&1 | tail -20'`
Expected: compiles. The module's functions are still unused (hooks land in Task 2) → `dead_code` warnings on `init`/`enqueue`/`mark_bound`/`on_poll` are acceptable here.

- [ ] **Step 6: Commit** — SKIP (no commits this session).

---

## Task 2: Wire the hooks

**Files:**
- Modify: `kernel/src/boot/log.rs` (`emit`)
- Modify: `kernel/src/net/mod.rs` (`init`, `poll`, DHCP bind block)

- [ ] **Step 1: Hook `emit()` in `log.rs`**

In `kernel/src/boot/log.rs`, in `emit()`, immediately after `crate::klog::push(bytes);` (around line 66), add:

```rust
        // dmesg ring buffer: always.
        crate::klog::push(bytes);
        #[cfg(feature = "netconsole")]
        crate::net::netconsole::enqueue(bytes);
```

- [ ] **Step 2: Hook `netconsole::init` in `net::init`**

In `kernel/src/net/mod.rs`, in `init()`, after the `if/else if/else` block that creates the device + DHCP socket and BEFORE `*NET.lock() = Some(NetState { ... })` (around line 79-80), add:

```rust
    } else {
        crate::bwarn!("net", "no Ethernet NIC found — loopback only");
    }

    #[cfg(feature = "netconsole")]
    netconsole::init(&mut net_sockets);

    *NET.lock() = Some(NetState {
```

- [ ] **Step 3: Hook `on_poll` drain in `net::poll`**

In `kernel/src/net/mod.rs`, in `poll()`, after the loopback poll and BEFORE the two ethernet `iface.poll` blocks (insert between line 104's loopback poll and line 106's comment), add:

```rust
        // Always poll loopback.
        let _ = net.iface_lo.poll(t, &mut net.dev_lo, &mut net.sockets);

        // Drain queued log lines into the UDP socket before the ethernet poll
        // below transmits them.
        #[cfg(feature = "netconsole")]
        netconsole::on_poll(&mut net.net_sockets);

        // Poll whichever Ethernet interface is active (virtio xor nic).
```

- [ ] **Step 4: Hook `mark_bound` on the DHCP bind edge**

In `kernel/src/net/mod.rs`, inside the DHCP handler, in the `if !net.dhcp_bound { ... }` block (around line 139-145), after `net.dhcp_bound = true;` and its logging, add the `mark_bound` call. The block becomes:

```rust
                        if !net.dhcp_bound {
                            net.dhcp_bound = true;
                            match router {
                                Some(gw) => crate::binfo!("net", "dhcp bound ip={} gw={}", addr.address(), gw),
                                None      => crate::binfo!("net", "dhcp bound ip={} gw=none", addr.address()),
                            }
                            #[cfg(feature = "netconsole")]
                            netconsole::mark_bound();
                        }
```

- [ ] **Step 5: Build feature-off**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | tail -20'`
Expected: compiles, ISO assembled, no new warnings (all netconsole code cfg-stripped).

- [ ] **Step 6: Build feature-on**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso CARGO_FEATURES=netconsole 2>&1 | tail -20'`
Expected: compiles. No more `dead_code` warnings on netconsole functions (all now called).

- [ ] **Step 7: Regression gate (feature-off)**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -20'`
Expected: default-NIC boot prints the Makefile success string. Netconsole off path unaffected.

- [ ] **Step 8: Commit** — SKIP.

---

## Task 3: Changelog + final builds

**Files:**
- Create: `CHANGELOG/355-26-06-08-netconsole-udp.md`

- [ ] **Step 1: Write the changelog**

Create `CHANGELOG/355-26-06-08-netconsole-udp.md`:

```markdown
# 355 — Netconsole UDP

**Data:** 2026-06-08

## Cosa
4° sink di log: streaming UDP broadcast (255.255.255.255:6666) di ogni riga
kernel INFO+, captabile con `nc -ul 6666`. Gate compile-time
`--features netconsole` (abilita anche `smoltcp/socket-udp`). `emit()` accoda
su un ring dedicato (mai locca NET → no deadlock col net poll task); il drain
+ send vive in `net::poll()`. Al primo bind DHCP viene spinto il backlog del
ring klog (32 KiB) → log da T+0 recuperati anche se la rete sale a ~T+3s.

## Perché
Debug su HW reale con seriale COM rotta. Netconsole dà log passivi/streaming
sulla LAN senza sessione SSH interattiva. Broadcast = zero-config (nessun IP
collector, nessun ARP/gateway). QEMU/VM non raggiungono la LAN fisica →
verifica solo HW reale.

## File toccati
- kernel/src/net/netconsole.rs (nuovo)
- kernel/src/net/mod.rs
- kernel/src/boot/log.rs
- kernel/Cargo.toml
- docs/superpowers/specs/2026-06-08-netconsole-udp-design.md
- docs/superpowers/plans/2026-06-08-netconsole-udp.md
```

- [ ] **Step 2: Final build both configs**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | tail -5 && make iso CARGO_FEATURES=netconsole 2>&1 | tail -5'`
Expected: both ISOs assemble.

- [ ] **Step 3: Real-hardware acceptance (manual — user-driven)**

Boot `build/os.iso` (built with `CARGO_FEATURES=netconsole`) on the real box.
On another LAN host run `nc -ul 6666` (Linux) / `ncat -ul 6666` (Windows). After
ruos logs `dhcp bound ip=...`, the listener should show the live log stream plus
the backlog starting at `[T+0.0..]`.

- [ ] **Step 4: Commit** — SKIP.

---

## Self-review notes

- **Spec coverage:** UDP broadcast sink (T1 `on_poll`), enqueue hook (T2.1), backlog flush on bind (T1 `mark_bound` + T2.4), compile-time gate (T1.1 feature + cfg on every item), no-deadlock via enqueue-only (T1 `enqueue` / `on_poll` split), no-log-in-drain (T1 `on_poll`), default build unchanged (T1.4/T2.5 feature-off builds). All present.
- **Type consistency:** `NcRing{buf,tail,len}` with `push`/`peek`/`advance`; statics `BOUND`/`NC_HANDLE`/`NC_RING`; fns `init(&mut SocketSet)`/`enqueue(&[u8])`/`mark_bound()`/`on_poll(&mut SocketSet)` — consistent across T1 and the T2 call sites. smoltcp 0.11 `udp::{Socket,PacketBuffer,PacketMetadata}`, `bind(u16)`, `send_slice(&[u8], IpEndpoint)`, `Ipv4Address::BROADCAST`.
- **Risk:** if smoltcp 0.11 refuses limited-broadcast egress from a plain udp socket, fall back to subnet-directed broadcast computed from the DHCP CIDR. DHCP itself egresses 255.255.255.255, so limited broadcast is expected to work.
- **Placeholder scan:** none.
```
