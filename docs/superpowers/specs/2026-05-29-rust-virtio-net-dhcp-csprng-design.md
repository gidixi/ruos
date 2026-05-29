# Rust virtio-net + DHCP + CSPRNG — Design Spec

**Date:** 2026-05-29
**Milestone:** Roadmap **Step 14** (Networking). Depends on Step 13 (PCI/ECAM, done)
for device discovery and on Step 6 (frame allocator) for DMA. First consumer of
the PCI layer; the DMA module it introduces is reused by Step 15 (AHCI).
**Status:** Design, ready for implementation planning.

## Context

The kernel already runs a `smoltcp` TCP/IP stack, but only over a **Loopback**
device (`net/mod.rs`, `net/loopback.rs`): `NetState { iface, device: Loopback,
sockets }`, IP `127.0.0.1/8`, driven by `net_poll_task` (executor, every 10 ms)
calling `net::poll()`. A socket pool (`net/sockets.rs`) and WASI socket host
functions exist and work over loopback (the wasm ping-pong demo). `net::init()`
runs in `boot/phases/userland.rs`.

There is **no real NIC**: no traffic leaves the VM. `random_get`
(`wasm/host/random.rs`) is a weak xorshift seeded from `TICKS` — explicitly
marked "Step 14 replaces with RDRAND-backed CSPRNG".

This step adds (1) a **virtio-net** driver so the stack reaches the outside world
(DHCP lease from QEMU's SLIRP), and (2) a **CSPRNG** seeded from `RDRAND`,
without breaking the existing loopback path or socket demos.

## Goals

- A reusable **DMA allocator** (`memory/dma.rs`) over the frame allocator
  (physically-contiguous regions, `virt = phys + HHDM`), backing the
  `virtio_drivers::Hal` trait. Reused by AHCI (Step 15).
- A **`map_io_range(phys, bytes)`** helper in `memory/mapper.rs` for multi-page
  MMIO windows (virtio BARs).
- A **virtio-net** device driver via the `virtio-drivers` crate (PCI transport),
  discovered through the Step 13 `pci` layer, adapted to `smoltcp::phy::Device`.
- A **second smoltcp `Interface`** for the NIC (Ethernet medium, DHCP), added to
  `NetState` alongside the existing loopback interface — loopback `127.0.0.1/8`
  behaviour is preserved unchanged.
- **DHCPv4 client** (`smoltcp` `dhcpv4::Socket`) that acquires a lease from
  QEMU's SLIRP and applies the IP + default gateway; logged on serial.
- A **CSPRNG** (`rng.rs`): `ChaCha20Rng` (crate `rand_chacha`) seeded from
  `RDRAND`. Kernel API `rng::fill(&mut [u8])`; rewires `random_get`; wires the
  VFS `/dev/random` node; optionally seeds smoltcp's TCP ISN.
- Boot-time smoke on serial; `make run-test` asserts the DHCP lease line.

## Non-goals (YAGNI)

- **No IRQ / MSI-X.** The NIC RX queue is polled by `net_poll_task` (10 ms),
  same cadence as today. MSI-X interrupt delivery is a separate future spec.
- **No IPv6, no DNS resolver, no TCP offload, no multi-NIC.** One virtio-net
  function; first match wins.
- **No virtio-blk.** Step 15 (AHCI) reuses `memory/dma.rs`; virtio-blk is not
  built here.
- **No ICMP echo server.** Outbound ping is not implemented; the smoke is the
  DHCP lease, not a ping reply (SLIRP headless ICMP is unreliable).
- **No hand-rolled virtqueue.** `virtio-drivers` owns the ring/transport.

## Architecture

```
pci (Step 13) ── find vendor 0x1AF4, class 0x02 ──▶ virtio-net BDF (discovery + log)
                                                          │
memory/dma.rs (NEW) ── impl virtio_drivers::Hal ──▶ virtio-drivers 0.13
   frame alloc + HHDM                                (VirtIONet + PciTransport)
memory/mapper.rs::map_io_range (NEW) ── BAR window ──▶     │
                                                          ▼
                                          net/virtio.rs (NEW)
                                   smoltcp::phy::Device adapter
                                   (RxToken/TxToken over VirtIONet)
                                                          │
net/mod.rs: NetState {
    iface_lo, dev_lo (Loopback, 127/8),     ── existing, untouched
    iface_net, dev_net (virtio, Ethernet),  ── NEW
    sockets (+ dhcpv4 socket handle),
}                                                         │
net_poll_task (10 ms) ── poll BOTH ifaces, handle dhcp event ──▶ lease → IP+route
                                                          │
rng.rs (NEW): RDRAND → ChaCha20Rng ──▶ random_get, /dev/random, (TCP ISN)
```

The boundary with `virtio-drivers`: the crate operates the virtio PCI device
itself (its `transport::pci` reads the device's virtio capabilities and maps BARs
via our `Hal::mmio_phys_to_virt` / `map_io_range`). Our Step 13 `pci` layer is
used to **discover** the device (find the `0x1AF4`/class-`0x02` function, enable
bus-master + memory-space via `enable_mmio`/`enable_bus_master`) and to obtain its
`PciAddress`/ECAM base, which is handed to `virtio-drivers`' PCI root.

## Components

### 1. `memory/dma.rs` (new) — DMA allocator + `Hal`

Physically-contiguous DMA regions from the frame allocator; `virt = phys + HHDM`
(uncached not required for virtio rings — they are normal RAM, coherent on x86;
do NOT set NO_CACHE for ring memory). API:

```rust
pub struct DmaRegion { pub phys: PhysAddr, pub virt: VirtAddr, pub pages: usize }
pub fn alloc(pages: usize) -> Option<DmaRegion>;   // contiguous frames
pub fn dealloc(r: DmaRegion);
```

`struct KernelHal;` implements `virtio_drivers::Hal`:
- `dma_alloc(pages, dir) -> (PhysAddr, NonNull<u8>)` via `dma::alloc`.
- `dma_dealloc(...)` via `dma::dealloc`.
- `mmio_phys_to_virt(paddr, size) -> NonNull<u8>` via `map_io_range`.
- `share`/`unshare`: identity on x86 (no IOMMU), return the paddr as-is.

> Plan verifies the exact `Hal` method signatures of virtio-drivers 0.13
> (associated `BufferDirection`, `PhysAddr`=usize, pointer types).

### 2. `memory/mapper.rs` (extension) — `map_io_range`

```rust
pub fn map_io_range(phys: PhysAddr, bytes: usize) -> Result<VirtAddr, MapError>;
```

Maps `ceil(bytes / 4096)` pages starting at `phys` (page-aligned down) with the
existing uncached MMIO flags (`map_io_page` flags), returns the virt of `phys`.
Idempotent per page. Used for virtio BAR windows.

### 3. `net/virtio.rs` (new) — driver + smoltcp Device adapter

- `find_and_init() -> Option<VirtioNet>`: locate the NIC via `crate::pci`
  (iterate `devices()` for `vendor_id == 0x1AF4 && class == 0x02`; if a
  `pci::find_vendor`-style helper is missing, add a thin one or iterate inline),
  `enable_mmio()` + `enable_bus_master()`, build a `virtio_drivers`
  `PciTransport` from the device + our ECAM base, then
  `VirtIONet::<KernelHal, PciTransport, QSIZE>::new(transport, BUF_LEN)`.
- `struct VirtioNet { inner: VirtIONet<...>, mac: [u8;6] }`.
- `impl smoltcp::phy::Device for VirtioNet`: `capabilities()` (medium Ethernet,
  max_transmission_unit = 1500), `receive()` → `Some((RxToken, TxToken))` when
  `inner.can_recv()`, `transmit()` → `TxToken`. Tokens copy between smoltcp's
  buffer and virtio-drivers' `receive()`/`send()` (which take/return owned
  `RxBuffer`/tx slices). RX buffer is recycled (`recycle_rx_buffer`) after the
  token consumes it.

### 4. `net/mod.rs` (extension) — second interface + DHCP

`NetState` gains the NIC interface and device; loopback fields keep working:

```rust
pub struct NetState {
    pub iface_lo:  Interface,          // was `iface`
    pub dev_lo:    loopback::Loopback, // was `device`
    pub iface_net: Option<Interface>,        // NEW (None if no NIC)
    pub dev_net:   Option<virtio::VirtioNet>,// NEW
    pub sockets:   SocketSet<'static>,
    pub dhcp:      Option<SocketHandle>,      // NEW (dhcpv4 socket)
}
```

`init()`:
- Build loopback iface as today (rename `iface`→`iface_lo`, `device`→`dev_lo`).
- If `virtio::find_and_init()` returns a NIC: build `iface_net` with
  `Config::new(HardwareAddress::Ethernet(mac))`, add a `dhcpv4::Socket` to the
  SocketSet, store its handle in `dhcp`. No static IP — DHCP assigns it.
- `bin/` socket demos that use `127.0.0.1` are unaffected (loopback iface).

`poll()`:
- `without_interrupts`, lock `NET`. Poll `iface_lo` against `dev_lo` (as today)
  AND `iface_net` against `dev_net` (if present), both with the same `sockets`.
- After polling, if `dhcp` is set, read the `dhcpv4::Socket` for
  `Event::Configured { config }` → `iface_net.update_ip_addrs` to `config.address`,
  set the default IPv4 route to `config.router`, and log once
  `net: dhcp bound ip=<addr> gw=<router>`. On `Event::Deconfigured` clear them.

### 5. `rng.rs` (new) — RDRAND → ChaCha20 CSPRNG

- `seed_from_rdrand() -> [u8;32]`: CPUID check for RDRAND
  (`core::arch::x86_64::__cpuid`, ECX bit 30); if absent → `panic!`/halt with a
  clear message (CLAUDE.md: never use the timer as entropy → no fallback). Fill
  32 bytes via `_rdrand64_step` with a bounded retry loop (10 retries per u64).
- Global `static RNG: Mutex<Option<ChaCha20Rng>>`, `init()` seeds it once at boot.
- `pub fn fill(buf: &mut [u8])` (locks, `rng.fill_bytes`). `pub fn next_u64()`.
- `random_get` (`wasm/host/random.rs`): replace the xorshift `next()` with
  `crate::rng::fill`. Keep the WASI signature.
- `/dev/random` VFS node: wire its `read` to `rng::fill` (the VFS device infra
  exists; if `/dev/random` is not yet registered, register it in `vfs/devices.rs`).
- Optional: seed smoltcp's interface `Config.random_seed` from `rng::next_u64()`
  so TCP ISNs are unpredictable.

### 6. `boot` wiring + `Makefile`

- `rng::init()` runs early in the boot sequence (after `arch`/`mem`, before any
  consumer; a natural slot is just before or inside the existing net/userland
  phase). `net::init()` already runs in `boot/phases/userland.rs`; the virtio
  discovery happens inside it (PCI is up by then — PCI phase precedes userland).
- `Makefile`: add `-netdev user,id=net0 -device virtio-net-pci,netdev=net0` to
  the `run` and `run-test` QEMU lines (preserve the existing `-machine q35`,
  `-device qemu-xhci`, timeout 120s, and PCI/shell assertions). Add the DHCP
  assertion to `run-test`.

## Data flow (DHCP lease)

```
boot: pci phase (Step 13) → userland phase → rng::init() ; net::init()
  net::init: find virtio (pci 0x1AF4/02) → enable mmio+busmaster → VirtIONet
             → iface_net (Ethernet, mac) + dhcpv4 socket
net_poll_task (10 ms):
  poll iface_lo/dev_lo  (127/8, unchanged)
  poll iface_net/dev_net:
     virtio TX: DHCP DISCOVER → SLIRP
     SLIRP → OFFER → virtio RX → smoltcp
     REQUEST → ACK
  dhcp socket Event::Configured{address:10.0.2.15/24, router:10.0.2.2}
     → iface_net IP + default route set
     → log "net: dhcp bound ip=10.0.2.15 gw=10.0.2.2"  (once)
```

## Error handling

- **No virtio NIC found:** log `net: no virtio-net (loopback only)`, continue —
  non-fatal (mirrors `pci::NoEcam`). Loopback + existing demos still work.
- **RDRAND absent:** fatal — `rng::init` halts with a clear message. No entropy
  fallback (security: CLAUDE.md).
- **DMA alloc failure / virtio init failure:** log and skip the NIC (non-fatal);
  the system boots without external networking rather than halting.
- **DHCP never completes:** no lease line is logged; `run-test` fails its grep
  (the gate catches a broken NIC/stack). Not a kernel error per se.

## Testing

`make run-test` (QEMU q35) gains `-netdev user,id=net0 -device
virtio-net-pci,netdev=net0`. Assertions (all must hold):
- existing shell sentinel `shell: init.sh complete`,
- existing PCI lines (`pci ... init ok devices>=1`, `xhci @`),
- **NEW** `net: dhcp bound ip=10.0.2.15 gw=10.0.2.2` (SLIRP's fixed lease).

This proves end-to-end: virtio-net TX/RX works, smoltcp processes Ethernet +
DHCP, the lease is applied. The loopback wasm ping-pong demo must remain green
(non-regression). CSPRNG: a boot log `rng: chacha20 seeded (rdrand)` confirms
seeding; `random_get`/`/dev/random` exercised by existing wasm tools.

## Decomposition into tasks (for the plan)

0. Add deps: `virtio-drivers = "0.13"`, `rand_chacha` (no_std, no `std`/`getrandom`
   default), extend `smoltcp` features with `socket-dhcpv4` (+ `medium-ethernet`).
1. `memory/mapper.rs`: `map_io_range`.
2. `memory/dma.rs`: `DmaRegion` + alloc/dealloc + `KernelHal: virtio_drivers::Hal`.
3. `rng.rs`: RDRAND seed + ChaCha20 + `fill`/`init`; rewire `random_get`; wire
   `/dev/random`; `rng::init()` in boot.
4. `net/virtio.rs`: discovery + `VirtIONet` + `smoltcp::phy::Device` adapter.
5. `net/mod.rs`: `NetState` second interface + dhcpv4 socket; `poll()` dual-iface
   + DHCP event → IP/route + lease log; rename `iface`/`device` → `_lo`.
6. `Makefile`: netdev + virtio-net-pci; DHCP assertion in `run-test`.
7. Docs: CHANGELOG entries, roadmap Step 14 → DONE.

## Open items for the implementation plan

- **Verify virtio-drivers 0.13 API**: exact `Hal` trait (method names/sigs,
  `BufferDirection`, `PhysAddr` type), `transport::pci::PciTransport`
  construction (it may need its own `PciRoot`/CAM over the ECAM base rather than
  our `pci_types` handle — reconcile the boundary), and `VirtIONet::new` +
  `receive`/`send`/`recycle_rx_buffer` signatures and the const `QSIZE`/`BUF_LEN`.
- **PCI discovery helper**: `pci::find_class(0x02, 0x00, _)` matches by class but
  prog_if varies; virtio-net is class `0x02`/sub `0x00`. If a vendor-based lookup
  is cleaner (`0x1AF4`), add a small `pci::find_vendor(vendor, class)` or iterate
  `pci::devices()` inline. Decide in the plan.
- **smoltcp dual-interface over one SocketSet**: confirm polling two `Interface`s
  against the same `SocketSet` in one `poll()` is correct (it is in smoltcp's
  model — each `poll` services sockets whose addressing the iface matches), and
  that loopback `127/8` traffic is unaffected by the Ethernet iface.
- **`rand_chacha` features**: ensure `default-features = false` (avoid `std`/
  `getrandom`); we seed manually from RDRAND.
- **/dev/random**: confirm the VFS device-node mechanism (`vfs/devices.rs`) can
  back a node with a closure/handler calling `rng::fill`; if not trivial, scope
  it as optional and keep `random_get` as the primary consumer.
```
