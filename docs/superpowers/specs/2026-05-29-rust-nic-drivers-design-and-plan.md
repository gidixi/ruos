# Real-Hardware NIC Drivers — Design Spec + Implementation Plan

**Date:** 2026-05-29
**Milestone:** Follow-up to Step 14 (Networking). Extends networking from the
paravirtual virtio-net (VM-only) to the most common real Ethernet controllers,
so the kernel gets a link on bare metal (Limine USB boot) as well as in VMs.
**Status:** Design + plan, ready for execution.
**Spec scope:** seven driver families, grouped by priority/effort:

- **Tier 1 (core, QEMU-testable):** Intel e1000 / e1000e, Realtek RTL8139.
- **Tier 1 (descriptor GbE):** Realtek RTL8169/8168/8111.
- **Tier 2 (high value):** Realtek RTL8125 (2.5G, extends RTL8169); Intel igb
  (I210/I211/I350 — the one *new* QEMU-testable family).
- **Tier 3 (real-HW, higher effort, optional):** Intel igc (I225/I226 2.5G);
  Broadcom tg3 (BCM57xx NetXtreme).

> **For agentic workers:** the "Implementation Plan" half uses checkbox
> (`- [ ]`) task tracking. Use superpowers:subagent-driven-development or
> superpowers:executing-plans to execute task-by-task.

---

## Context

After Step 13 (PCIe/ECAM) and Step 14 (virtio-net + DHCP + CSPRNG) the kernel
has: a `pci` layer (`find_class`, `PciDevice { bdf, .. }`, `bar(n)`,
`enable_mmio`, `enable_bus_master`); a contiguous DMA allocator
(`memory::dma::alloc` → `DmaRegion`, `frames::allocate_contiguous`,
`map_io_range`, `mapper::hhdm_virt`); a `smoltcp::phy::Device` adapter pattern
(`net/virtio.rs`) with a `NetState` polling multiple interfaces against one
`SocketSet` plus a DHCPv4 client; and a ChaCha20 CSPRNG.

virtio-net is paravirtual — it exists only inside a hypervisor. On real hardware
the kernel enumerates a genuine NIC over PCI but has nothing to bind. This spec
adds the drivers that cover the overwhelming majority of physical machines.

**Two strategic notes that shape the plan:**

1. **The cheapest real-HW path is to carry a supported NIC.** A PCIe add-in card
   with a chip already covered here — an Intel 82574L (e1000e) or an RTL8168 —
   costs ~€10–15 and works in any desktop with a free PCIe slot, with zero new
   drivers. This is the recommended primary bare-metal test method; the onboard
   drivers below are for testing *unmodified* machines.
2. **Drive the list from real hardware.** `lspci -nn | grep -iE 'ethernet|network'`
   on each test box gives the exact `vendor:device`. The probe table below
   covers the common IDs; add the specific one a given machine reports rather
   than guessing.

**Why these seven cover "the most used":** Intel onboard LAN of the last ~15
years (8254x → ICH → PCH, incl. I217/I218/**I219**) is driven by **e1000e**;
Realtek onboard LAN (the ubiquitous "Realtek PCIe GbE Family Controller",
RTL8111/8168) is the **RTL8169** family. Those two alone cover most physical PCs.
RTL8139 adds the legacy/VM case. RTL8125 + igb/igc add modern 2.5G and Intel
server/workstation gigabit; tg3 adds Broadcom business/enterprise machines.

### QEMU testability matrix (decides task ordering)

QEMU's NIC model list (8.x) includes `e1000`, `e1000e`, `rtl8139`, and — since
QEMU 8.0 — `igb`, but **no** `rtl8169`/`8168`, `rtl8125`, `igc`, or Broadcom.

| Family | QEMU device | Automated `make run-test` gate? |
|--------|-------------|----------------------------------|
| e1000 (82540EM) | `-device e1000` | ✅ |
| e1000e (82574L) | `-device e1000e` | ✅ |
| RTL8139 | `-device rtl8139` | ✅ |
| igb (I210/I211/I350; QEMU models 82576) | `-device igb` | ✅ |
| RTL8169/8168/8111 | *(not emulated)* | ❌ real HW only |
| RTL8125 (2.5G) | *(not emulated)* | ❌ real HW only |
| igc (I225/I226) | *(not emulated)* | ❌ real HW only |
| Broadcom tg3 (BCM57xx) | *(not emulated)* | ❌ real HW only |

Four families get the automated DHCP gate; the rest are compile-gated + ride on
shared, already-tested ring code + manual bare-metal validation.

---

## Prior art — existing Rust implementations (survey)

**Intel e1000 / e1000e**
- **`rcore-os/e1000-driver`** — strongest reuse candidate. An e1000 driver in
  Rust for the Intel 82540EP/EM and 82574L, supporting e1000 and e1000e on QEMU
  and on physical hardware. Generic over a host trait: implement
  `e1000_driver::e1000::KernelFunc` (DMA alloc, phys/virt translation, delay),
  build `E1000Device::<K>::new(regs)`, then `e1000_transmit`/`e1000_recv`. That
  boundary maps near-1:1 onto ruos `dma`/`mapper`/`timer`. Forks: `elliott10`,
  `lispking`, `451846939`.
- **`fujita/rust-e1000`** — reference only; built for Rust-for-Linux PCI/DMA/net
  abstractions, compiled against a Linux fork, so Linux-bound (C bindings).

**Realtek RTL8139** — **`vgarleanu/rtl8139-rs`** (no_std, sync + async).
**`Dentosal/rust_os` `driver_rtl8139`** follows the OSDev wiki and references the
SerenityOS/Linux/u-boot drivers (DMARegion + 4-slot TX). Good native references.

**Realtek RTL8169/8168/8111/8125** — no ready no_std Rust crate. Primary source:
OSDev RTL8169 wiki; an OSDev forum thread notes the RTL8139C+ is nearly identical
in driver logic to the RTL8169 (descriptor model). For RTL8125, the Linux
mainline `r8169` driver supports the RTL8125 family — i.e. it is a register/ID
extension of the same descriptor engine, not a new architecture.

**Intel igb (I210/I211/I350)** — Intel SDM / public I210 datasheet; Linux `igb`.
Rust reference for Intel advanced descriptors + MSI-X: **`ixy-languages` ixy.rs**
(userspace `ixgbe`/virtio educational driver) — read for descriptor/MSI-X
patterns, not drop-in.

**Intel igc (I225/I226)** — Linux `igc`; Intel I225/I226 datasheet. No known
no_std Rust driver. Register set close to igb but distinct.

**Broadcom tg3 (BCM57xx)** — Linux `tg3.c` and the public Broadcom NetXtreme
programmer's reference manual. The hardest target (large init, windowed register
access). No known no_std Rust driver.

**Build vs adopt decision:**
1. **e1000/e1000e:** adopt `rcore-os/e1000-driver` via a ruos `KernelFunc` impl
   (gets e1000e for free) *unless* the trait fits awkwardly — then native, using
   it as reference. Confirm licence.
2. **RTL8139:** native (tiny), referencing `rtl8139-rs` + Dentosal.
3. **RTL8169/8125:** native on the shared `ring.rs`; 8125 = 8169 + IDs/init delta.
4. **igb/igc:** native, advanced-descriptor variant of `ring.rs`; ixy.rs as ref.
5. **tg3:** native from Linux `tg3.c`/Broadcom PRM; last and optional.

---

## Goals

- A `net/nic/` sub-tree: one module per family + a shared descriptor-ring engine,
  each exposing the chip as a `smoltcp::phy::Device` via the contract
  `net/virtio.rs` already satisfies.
- PCI binding by `(vendor, device)`/class via the Step 13 layer, with a probe
  table that selects the right driver and logs the exact device id.
- MMIO via `map_io_range` (memory BAR) or PIO (RTL8139 I/O BAR); DMA rings/buffers
  via `memory::dma`; descriptor coherency via fences.
- DHCP-over-Ethernet for e1000, e1000e, RTL8139, igb in QEMU via the existing
  `NetState`/poll loop (poll model, no IRQ initially).
- RTL8169/8125, igc, tg3 implemented and structured for bare-metal validation,
  reusing the shared ring engine proven by the QEMU-testable drivers.
- `make run-test` gains a parameterised per-NIC DHCP gate.

## Non-goals (YAGNI)

- No interrupt-driven RX/TX initially (poll each timer tick reading the chip ISR);
  MSI/MSI-X/INTx is a later enhancement gated on the MSI spec. (igb/igc/tg3 want
  MSI-X eventually; rtl8139 uses INTx.)
- No checksum/TSO/LRO offload, no jumbo frames, no multi-queue/RSS, no flow
  control. One RX + one TX ring/queue per NIC; smoltcp does software checksums.
- No EEPROM/NVM writes; no PHY tuning beyond link-up; rely on defaults.
- No 5G/10G (RTL8126/8127, Aquantia), no Atheros/Marvell/nForce/VIA, no WiFi,
  no hot-unplug, no power management.

---

## Architecture

```
pci::devices() ── probe table (vendor/device → driver) ──┐
                                                          ▼
                       net/nic/mod.rs : Nic enum + probe_and_init()
   ┌──────────┬──────────┬──────────┬──────────┬──────────┬──────────┬──────────┐
   ▼          ▼          ▼          ▼          ▼          ▼          ▼
 e1000.rs  rtl8139.rs rtl8169.rs  igb.rs     igc.rs     tg3.rs   (RTL8125 =
 Intel GbE  Realtek    Realtek    Intel GbE  Intel 2.5G  Broadcom  rtl8169.rs
 (legacy    10/100     GbE/2.5G   (advanced  (advanced   NetXtreme + IDs/delta)
 desc.)     register   (desc.)    desc.+     desc.)      (windowed
   │        based)       │        MSI-X)        │         regs)
   │                     │          │           │           │
   ├── legacy 16B desc ──┤          ├── advanced (16B) desc ─┤
   │   via ring.rs       │          │   via ring.rs (adv)    │
   └─────────────────────┴──────────┴────────────────────────┘
                          │
        memory::dma::alloc / map_io_range / mapper::hhdm_virt  (Step 14)
                          │
                          ▼
   each driver impls smoltcp::phy::Device ─▶ NetState (Step 14): another
   Ethernet iface + DHCP socket, polled by net_poll_task. RTL8139 has no ring.
```

`ring.rs` provides two descriptor layouts behind one index/OWN/EOR engine:
**legacy 16-byte** (e1000) and **advanced 16-byte** (igb/igc; split read/writeback
formats). RTL8169/8125 use their own 16-byte descriptor (OWN+EOR) — same engine,
chip-specific field writes. tg3 uses producer/consumer ring indices in a status
block — a thin tg3-local ring, not `ring.rs` (documented in its section).

---

## Components

### 0. `net/nic/mod.rs` — probe table + dispatch

```rust
pub enum Nic {
    E1000(e1000::E1000), Rtl8139(rtl8139::Rtl8139), Rtl8169(rtl8169::Rtl8169),
    Igb(igb::Igb), Igc(igc::Igc), Tg3(tg3::Tg3),
}
pub fn probe_and_init() -> Option<Nic>;   // first supported NIC, initialized
pub fn mac(&self) -> [u8; 6];
// Nic forwards smoltcp::phy::Device to the active variant (enum dispatch).
```

Probe table (PCI vendor/device → driver; bind + log exact id):

| Vendor | Device(s) | Driver |
|--------|-----------|--------|
| `0x8086` Intel | `0x100E` 82540EM, `0x1004`/`0x100F`/`0x10D3*` | e1000 |
| `0x8086` | `0x10D3` 82574L, ICH/PCH I217/I218/**I219** ids | e1000e (same driver, init delta) |
| `0x8086` | `0x1521` I350, `0x1533`/`0x1531` I210, `0x1539` I211, `0x10C9`/`0x150A` 82576 | igb |
| `0x8086` | `0x15F2`/`0x15F3` I225, `0x125B`/`0x125C` I226 | igc |
| `0x10EC` Realtek | `0x8139` | rtl8139 |
| `0x10EC` | `0x8161`/`0x8167`/`0x8168`/`0x8169`/`0x8136` | rtl8169 |
| `0x10EC` | `0x8125`/`0x3000` (RTL8125) | rtl8169 (8125 path) |
| `0x14E4` Broadcom | BCM57xx: `0x1677` 5751, `0x1681` 5764, `0x1692` 57780, etc. | tg3 |

Realtek vs Realtek: device `0x8139` → register driver; everything else `0x10EC`
network-class → descriptor driver (8169/8125), keyed by id for the 8125 delta.
`net/mod.rs::init()` calls `nic::probe_and_init()` where it now calls
`virtio::VirtioNet::find_and_init()` (keep both; prefer virtio in a VM, else first
real NIC — rule fixed in plan).

### 1. `net/nic/ring.rs` — shared descriptor-ring engine

A ring of N descriptors in one `DmaRegion` + per-slot 2 KiB DMA buffers, with
head/tail indices and OWN/EOR semantics. Two descriptor flavours:
- **legacy** (e1000, rtl8169/8125): 16 B, simple status/length + buffer addr.
- **advanced** (igb/igc): split *read* (buffer/header addrs) and *writeback*
  (status/length/checksum) 16 B formats.

x86 is DMA-coherent → ring/buffer memory is **normal cacheable RAM** (never
NO_CACHE). Use `mbarrier`/`core::sync::atomic::fence` around OWN-bit handoff so
the descriptor write isn't reordered past the tail-pointer write. The engine owns
indices + OWN/EOR; each driver does the chip-specific field writes via volatile
access into `desc.virt`.

### 2. `net/nic/e1000.rs` — Intel 82540EM / 82574L (e1000 / e1000e)

MMIO BAR0. Key registers (8254x SDM): CTRL 0x0000 (RST 1<<26, SLU 1<<6),
STATUS 0x0008, EERD 0x0014 (MAC if not in RAL/RAH), ICR/IMS/IMC 0x00C0/D0/D8,
RCTL 0x0100 (EN 1<<1, BAM, BSIZE, SECRC), TCTL 0x0400 (EN 1<<1, PSP),
TIPG 0x0410, RDBAL/RDBAH/RDLEN/RDH/RDT 0x2800.., TDBAL/TDBAH/TDLEN/TDH/TDT
0x3800.., RAL/RAH 0x5400/5404, MTA 0x5200.
Init: reset → SLU → zero MTA → read MAC → alloc rings (`ring.rs` legacy) →
program RD*/TD* → RCTL.EN + TCTL.EN. TX: buffer + len + CMD(EOP|IFCS|RS), bump
TDT. RX: poll DD bit, copy, recycle, bump RDT. e1000e (82574L) = same driver,
minor init delta (it prefers MSI-X but works with the poll/legacy-int path).
**Adopt-rcore option:** impl `KernelFunc` for ruos, wrap `E1000Device` in the
`phy::Device` adapter — covers both e1000 and e1000e.

### 3. `net/nic/rtl8139.rs` — Realtek RTL8139 (register-based)

No descriptor ring. I/O BAR (PIO) or MMIO BAR. Registers (datasheet/OSDev):
IDR0-5 0x00 (MAC), TSD0-3 0x10.., TSAD0-3 0x20.. (TX buffer phys), RBSTART 0x30
(RX ring phys), CR 0x37 (RST 0x10, RE 0x08, TE 0x04), CAPR 0x38, IMR/ISR
0x3C/0x3E (ROK 1, TOK 4), RCR 0x44 (AB|AM|APM|AAP | WRAP 1<<7), CONFIG1 0x52.
Init: bus-master on, CONFIG1=0, soft reset (CR.RST, wait clear), RBSTART→8K+16
(+1500 WRAP) DMA ring, IMR=TOK|ROK, RCR=AB|AM|APM|AAP|WRAP (+ RBLEN), CR=RE|TE.
TX: 4 round-robin TSAD/TSD slots. RX: single circular buffer, 4-byte
header(status+len) per packet, advance via CAPR.
**Real-HW gotcha:** set RCR RBLEN/FIFO-threshold or a physical chip may treat the
buffer as zero-sized and never DMA in (classic works-in-QEMU-fails-on-metal).

### 4. `net/nic/rtl8169.rs` — Realtek RTL8169/8168/8111 (+ RTL8125 path)

MMIO BAR, descriptor rings (`ring.rs` legacy-style with OWN+EOR). Registers
(OSDev RTL8169): IDR0-5 0x00 (MAC, 8-bit reads), CR 0x37 (RST; RE/TE), TCR/RCR
0x40/0x44, IMR/ISR 0x3C/0x3E, 9346CR 0x50 (unlock 0xC0 / lock 0x00),
CONFIG1 0x52, RMS 0xDA (rx max; 0 = accept none, keep 0x1FFF), MTPS 0xEC (tx max,
≤0x3B), TNPDS 0x20 (TX ring phys, 8-byte aligned), RDSAR 0xE4 (RX ring phys).
Init: reset → unlock config → read MAC → set RMS/MTPS → alloc TX/RX rings →
program TNPDS/RDSAR → TCR/RCR → IMR=TOK|ROK → lock config → CR=RE|TE.
Descriptor: 4 dwords (flags incl. OWN/EOR/length, vlan, buf lo, buf hi); last has
EOR.
**RTL8125 (2.5G) delta:** same descriptor engine; additional device IDs; extra
init — the 8125 needs its "RX/TX config + PHY ups" sequence (mirror Linux r8169's
`rtl8125` path: set the additional MAC config registers, larger RX descriptor
fetch). Implement as a `variant: Rtl816xKind` flag in this module, not a new file.

### 5. `net/nic/igb.rs` — Intel I210/I211/I350 (QEMU 82576)

MMIO BAR0, **advanced descriptors + MSI-X-oriented** controller (poll path works).
Init is e1000-shaped but with the igb register map and queue setup: CTRL reset,
read MAC from RAL/RAH, set up RX queue 0 (RDBAL/RDBAH/RDLEN/RDH/RDT + SRRCTL
descriptor type/buffer size), TX queue 0 (TDBAL.. + TXDCTL queue enable),
RCTL/TCTL enable, GPIE/IMS for interrupts (left masked in poll mode). Use the
**advanced** descriptor flavour of `ring.rs`. Reference: I210 datasheet + Linux
`igb`; ixy.rs for the advanced-descriptor/queue-enable pattern. QEMU `-device
igb` (models 82576) is the automated gate.

### 6. `net/nic/igc.rs` — Intel I225/I226 (2.5G)

Register set close to igb (advanced descriptors, per-queue TXDCTL/RXDCTL enable),
distinct offsets and the I225/I226 reset/PHY quirks (the well-documented I225
errata mean a clean reset + link-up sequence matters). Reuse the advanced `ring.rs`
flavour. Reference: Linux `igc` + I225/I226 datasheet. **Not QEMU-emulated** →
compile gate + bare-metal validation on an I225/I226 board.

### 7. `net/nic/tg3.rs` — Broadcom BCM57xx NetXtreme

The hardest. Windowed/indirect register access (MEMWIN), firmware handshake
(`NIC_SRAM` mailbox), reset via `MISC_HOST_CTRL`/`GRC` and the
producer/consumer-ring + **status block** DMA model (not the simple OWN/EOR ring,
so tg3 keeps its own thin ring locally rather than `ring.rs`). MAC from
`MAC_ADDR` registers / NVRAM. Reference: Linux `tg3.c` + Broadcom NetXtreme
programmer's reference manual. **Not QEMU-emulated** → compile gate + bare-metal
validation on a Broadcom machine. Optional / last; ship the others first.

### 8. `net/mod.rs` + `Makefile`

`net/mod.rs`: add `pub mod nic;`; in `init()`, after the virtio probe, try
`nic::probe_and_init()`; build a second Ethernet `Interface` + DHCP socket exactly
as Step 14 does. Poll loop + DHCP handling unchanged.
`Makefile`: `NIC ?= e1000` → `-device $(NIC),netdev=net0`; `run-test` runs the
DHCP gate for `e1000`, `e1000e`, `rtl8139`, `igb`. Non-QEMU chips excluded from
the gate, marked HW-validated in docs.

## Error handling

`nic::NicError { NoDevice, UnsupportedDevice(u16), BarMissing, ResetTimeout,
LinkDown, Dma }` + one-line `Display` (matches existing modules). Reset/link
waits are bounded against `timer::ticks()` (no infinite loops) → timeout logged
via `bwarn!`, NIC skipped (loopback survives), never a panic. DMA failure → `Dma`.

## Testing strategy

`no_std` kernel, no host `cargo test`; tests are QEMU boot + serial grep under
`make run-test`. Per-chip DHCP gate (one launch per QEMU-testable chip): each logs
`net: <chip> mac=..` + `net: dhcp bound ip=10.0.2.15`. Non-regression: virtio +
loopback still work. Non-QEMU chips: compile + ride shared `ring.rs` + manual
bare-metal boot (document procedure). `-cpu max` stays (RDRAND for CSPRNG).

---

# Implementation Plan

> All commands via WSL (per `CLAUDE.md`):
> ```bash
> wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && <cmd>'
> ```
> Build `make build`; gate `make run-test` (120 s, asserts shell sentinel + PCI +
> per-NIC DHCP). Logging `binfo!("tag", ...)` / `bwarn!`. **Branch:** create and
> commit this spec on `feature/nic-drivers`, then work there; commit per task; do
> NOT push. **CHANGELOG:** one `CHANGELOG/NN-26-05-29-slug.md` per task; the `NN`
> shown (140+) is indicative — take the next free number at execution time. Keep
> slugs (Italian `## Cosa / ## Perché / ## File toccati`).

**API-drift caution.** Verify against installed sources, not memory:
`rcore-os/e1000-driver` `KernelFunc`/`E1000Device` (if adopted); your Step 13
`pci` API (`find_class`, `PciDevice { bdf, .. }` field names — earlier specs used
`dev.bdf.bus`, the Step 14 plan used `dev.address.bus()`; **pick the real one and
use it consistently**); your Step 14 `dma`/`map_io_range`/`hhdm_virt` and
`NetState`/`net_poll_task` names; smoltcp 0.11 `phy::Device` GAT signatures and
`dhcpv4` API. The chip register details live in the spec sections above and the
referenced datasheets/Linux drivers — do not invent register offsets.

## File structure

| File | Responsibility | C/M |
|------|----------------|-----|
| `kernel/Cargo.toml` | (opt) `e1000-driver` dep if adopting rcore | M |
| `kernel/src/net/nic/mod.rs` | probe table, `Nic` enum, dispatch | C |
| `kernel/src/net/nic/ring.rs` | shared descriptor-ring engine (legacy + advanced) | C |
| `kernel/src/net/nic/e1000.rs` | Intel e1000/e1000e | C |
| `kernel/src/net/nic/rtl8139.rs` | Realtek RTL8139 | C |
| `kernel/src/net/nic/rtl8169.rs` | Realtek RTL8169/8168/8111 + RTL8125 path | C |
| `kernel/src/net/nic/igb.rs` | Intel I210/I211/I350 | C |
| `kernel/src/net/nic/igc.rs` | Intel I225/I226 | C |
| `kernel/src/net/nic/tg3.rs` | Broadcom BCM57xx | C |
| `kernel/src/net/mod.rs` | second iface + DHCP; prefer virtio else NIC | M |
| `Makefile` | `NIC ?=` param; per-chip DHCP gate | M |
| `docs/superpowers/{specs,plans}/…` , `README.md`, roadmap | docs | M |

---

## Task 1 — Branch, `nic` module skeleton, probe table

**Files:** Create `net/nic/mod.rs`; modify `net/mod.rs` (`pub mod nic;`).

- [ ] **Step 1:** `git checkout -b feature/nic-drivers`; commit this spec under
  `docs/superpowers/specs/`.
- [ ] **Step 2:** Create `net/nic/mod.rs` with the `Nic` enum (variants stubbed),
  `NicError` (+`Display`), and the probe table (vendor/device → kind) from
  Component 0. `probe_and_init()` for now scans `pci::devices()`, matches the
  table, logs `binfo!("nic", "found {:04x}:{:04x} -> {:?}", v, d, kind)`, returns
  `None` (no driver yet). Add `pub mod nic;` to `net/mod.rs`.
- [ ] **Step 3 (test):** `make build` → `Finished` (dead-code warnings OK).
- [ ] **Step 4:** `CHANGELOG/140-26-05-29-nic-skeleton-probe.md` + commit
  (`feat(net): nic module skeleton + PCI probe table`).

## Task 2 — `net/nic/ring.rs` shared descriptor engine

**Files:** Create `net/nic/ring.rs`; modify `net/nic/mod.rs` (`mod ring;`).

- [ ] **Step 1:** Implement `DescRing` (N descriptors in one `DmaRegion` + N
  buffers; head/tail; OWN/EOR; `desc_phys()`), with a `legacy` and an `advanced`
  descriptor accessor. Fences around OWN handoff. Cacheable RAM (no NO_CACHE).
- [ ] **Step 2 (test):** `make build` → `Finished`.
- [ ] **Step 3:** `CHANGELOG/141-26-05-29-nic-ring-engine.md` + commit
  (`feat(net): shared descriptor-ring engine`).

## Task 3 — Intel e1000 (decide adopt-rcore vs native) + `phy::Device`

**Files:** Create `net/nic/e1000.rs`; modify `mod.rs`; maybe `Cargo.toml`.

- [ ] **Step 1 (spike):** Try `impl e1000_driver::e1000::KernelFunc` over
  `dma`/`mapper`/`timer`. If clean → adopt (add dep, wrap `E1000Device`). If
  awkward → native using the Component 2 register table + `ring.rs` legacy.
- [ ] **Step 2:** Implement init/TX/RX, MAC read, and `impl smoltcp::phy::Device`
  (copy-into-`Vec` RX + recycle, like `net/virtio.rs`). Wire into the `Nic` enum.
- [ ] **Step 3 (test, compile):** `make build` → `Finished` (gate added in Task 4).
- [ ] **Step 4:** `CHANGELOG/142-26-05-29-e1000-driver.md` + commit
  (`feat(net): Intel e1000 driver`).

## Task 4 — `net/mod.rs` wiring + Makefile NIC param + e1000 DHCP gate

**Files:** Modify `net/mod.rs`, `Makefile`.

- [ ] **Step 1:** In `net::init()`, after the virtio probe, call
  `nic::probe_and_init()`; if `Some`, add a second Ethernet `Interface` + dhcpv4
  socket (reuse the Step 14 code path). Fix the prefer-virtio-else-NIC rule.
- [ ] **Step 2:** `Makefile`: `NIC ?= e1000`; add `-device $(NIC),netdev=net0`
  to `run`/`run-test`; in `run-test` assert
  `grep -qE "net .* dhcp bound ip=10\.0\.2\.15"`.
- [ ] **Step 3 (test, headline):**
  `make run-test 2>&1 | tail -6` → `TEST_PASS`; serial has `net: e1000 mac=..`
  and `net: dhcp bound ip=10.0.2.15`. Debug: confirm `pci init ok devices=`
  increased and the probe logged the e1000 id.
- [ ] **Step 4:** `CHANGELOG/143-26-05-29-net-nic-wiring-e1000-gate.md` + commit
  (`feat(net): wire NIC iface + DHCP; e1000 QEMU gate`).

## Task 5 — e1000e binding + QEMU gate

**Files:** Modify `net/nic/e1000.rs` (if native), `Makefile`.

- [ ] **Step 1:** Bind `0x10D3` (82574L) to the e1000 driver; apply the e1000e
  init delta (rcore handles it automatically). Log `net: e1000e mac=..`.
- [ ] **Step 2 (test):** run-test with `NIC=e1000e`:
  `make run-test NIC=e1000e 2>&1 | tail -6` → `TEST_PASS` + DHCP bound. Keep the
  default `NIC=e1000` gate too (loop both, or two targets).
- [ ] **Step 3:** `CHANGELOG/144-26-05-29-e1000e-gate.md` + commit
  (`feat(net): e1000e (82574L) support + gate`).

## Task 6 — Realtek RTL8139 + QEMU gate

**Files:** Create `net/nic/rtl8139.rs`; modify `mod.rs`.

- [ ] **Step 1:** Native register driver (Component 3): I/O or MMIO BAR, reset,
  CONFIG1, RBSTART RX ring, 4 TX slots, IMR/RCR (incl. RBLEN real-HW note),
  CR=RE|TE; `impl phy::Device`. Wire into `Nic`.
- [ ] **Step 2 (test):** `make run-test NIC=rtl8139` → `TEST_PASS` +
  `net: rtl8139 mac=..` + DHCP bound.
- [ ] **Step 3:** `CHANGELOG/145-26-05-29-rtl8139-driver-gate.md` + commit
  (`feat(net): RTL8139 driver + QEMU gate`).

## Task 7 — Intel igb (I210/I211/I350) + QEMU gate

**Files:** Create `net/nic/igb.rs`; modify `mod.rs`.

- [ ] **Step 1:** Native driver (Component 5) on the **advanced** `ring.rs`
  flavour: CTRL reset, MAC from RAL/RAH, RX/TX queue 0 setup (SRRCTL/TXDCTL
  enable), RCTL/TCTL, interrupts masked (poll). `impl phy::Device`. Reference
  ixy.rs for the queue-enable/advanced-descriptor shape.
- [ ] **Step 2 (test):** `make run-test NIC=igb` → `TEST_PASS` + `net: igb mac=..`
  + DHCP bound. (QEMU igb models the 82576.)
- [ ] **Step 3:** `CHANGELOG/146-26-05-29-igb-driver-gate.md` + commit
  (`feat(net): Intel igb (I210/I211/I350) driver + QEMU gate`).

## Task 8 — Realtek RTL8169/8168/8111 (descriptor) — compile + structure

**Files:** Create `net/nic/rtl8169.rs`; modify `mod.rs`.

- [ ] **Step 1:** Native driver (Component 4) on `ring.rs` legacy+OWN/EOR: reset,
  unlock config (9346CR), MAC from IDR0-5, RMS/MTPS, TNPDS/RDSAR rings, TCR/RCR,
  lock, CR=RE|TE. `variant: Rtl816xKind` field. `impl phy::Device`.
- [ ] **Step 2 (test, compile only — not QEMU-emulated):** `make build` →
  `Finished`. Add no gate.
- [ ] **Step 3 (bare-metal, manual):** boot the Limine USB on the physical
  RTL8168 box; confirm over serial `net: rtl8169 mac=..` + a DHCP lease on the
  real LAN. Record the result in the CHANGELOG.
- [ ] **Step 4:** `CHANGELOG/147-26-05-29-rtl8169-driver.md` + commit
  (`feat(net): RTL8169/8168/8111 descriptor driver`).

## Task 9 — RTL8125 (2.5G) extension of RTL8169

**Files:** Modify `net/nic/rtl8169.rs`, `net/nic/mod.rs`.

- [ ] **Step 1:** Add `0x8125`/`0x3000` to the probe → `Rtl816xKind::Rtl8125`;
  implement the 8125 init delta (extra MAC config regs + descriptor-fetch tweaks,
  mirroring Linux `r8169` `rtl8125` path). Log `net: rtl8125 mac=..`.
- [ ] **Step 2 (test, compile):** `make build` → `Finished`.
- [ ] **Step 3 (bare-metal, manual, if an 8125 board is available):** confirm
  link + DHCP over serial; record in CHANGELOG.
- [ ] **Step 4:** `CHANGELOG/148-26-05-29-rtl8125-2_5g.md` + commit
  (`feat(net): RTL8125 2.5G support (RTL8169 family extension)`).

## Task 10 (OPTIONAL) — Intel igc (I225/I226)

**Files:** Create `net/nic/igc.rs`; modify `mod.rs`.

- [ ] **Step 1:** Native driver (Component 6) on advanced `ring.rs`; igc offsets,
  clean reset + link-up (I225 errata), per-queue enable. `impl phy::Device`.
- [ ] **Step 2 (test, compile):** `make build` → `Finished` (not QEMU-emulated).
- [ ] **Step 3 (bare-metal, manual):** validate on an I225/I226 board; record.
- [ ] **Step 4:** `CHANGELOG/149-26-05-29-igc-i225-i226.md` + commit
  (`feat(net): Intel igc (I225/I226) driver`).

## Task 11 (OPTIONAL, hardest, last) — Broadcom tg3

**Files:** Create `net/nic/tg3.rs`; modify `mod.rs`.

- [ ] **Step 1:** Native driver (Component 7): windowed register access, GRC/MISC
  reset + firmware handshake, MAC from MAC_ADDR/NVRAM, producer/consumer rings +
  status block (tg3-local, not `ring.rs`), enable RX/TX. `impl phy::Device`.
  Reference Linux `tg3.c` + Broadcom PRM.
- [ ] **Step 2 (test, compile):** `make build` → `Finished`.
- [ ] **Step 3 (bare-metal, manual):** validate on a Broadcom box; record.
- [ ] **Step 4:** `CHANGELOG/150-26-05-29-tg3-broadcom.md` + commit
  (`feat(net): Broadcom tg3 (BCM57xx) driver`).

## Task 12 — Docs + roadmap

**Files:** `docs/superpowers/plans/…`, `README.md`, roadmap.

- [ ] **Step 1:** Commit the implementation plan under `plans/`; update
  `README.md` layout (new `net/nic/` tree) and the roadmap (real-HW NIC support).
- [ ] **Step 2:** Add a `docs/` bare-metal validation note (the lspci-driven
  procedure + serial-capture setup for the non-QEMU chips).
- [ ] **Step 3:** `CHANGELOG/151-26-05-29-nic-docs-roadmap.md` + commit
  (`docs(net): NIC drivers plan, README, roadmap, bare-metal note`).

---

## Done criteria

- `make run-test` → `TEST_PASS` with serial containing DHCP leases for **e1000,
  e1000e, rtl8139, igb** (`net: <chip> mac=..` + `net: dhcp bound ip=10.0.2.15`),
  plus unchanged virtio + loopback + shell + PCI gates.
- RTL8169/8168/8111, RTL8125, igc, tg3 compile, share the proven ring engine
  where applicable, and have a documented bare-metal validation path.
- `phy::Device` boundary unchanged; adding a NIC is additive to `NetState`.

## Notes for the implementer

- **Sequence by value:** Tasks 1–7 (through igb) are all QEMU-testable and cover
  the bulk of real machines via the carry-a-card strategy + the common Intel/
  Realtek onboard chips. Do those first. Tasks 8–11 are real-HW; ship 8–9
  (Realtek GbE/2.5G) before the optional 10–11 (igc/tg3).
- **MSI later:** the poll path works for all of these in QEMU and likely on metal.
  Interrupt mode (e1000e/igb/igc → MSI-X; rtl8139 → INTx) waits on the MSI spec.
- **tg3 is a project on its own** — windowed regs + firmware handshake + status
  block. Treat it as optional; the others deliver "most used" without it.
- **Bare-metal logging:** confirm the test machine exposes a serial port (or use a
  PCIe/USB serial) so `make run-test`-style serial assertions are observable off
  the QEMU path.
