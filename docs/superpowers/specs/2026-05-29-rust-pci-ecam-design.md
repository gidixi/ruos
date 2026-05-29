# Rust PCIe Enumeration (ECAM) — Design Spec

**Date:** 2026-05-29 (rev. 2026-05-29 — `pci_types` hybrid, see Prior art)
**Milestone:** Roadmap **Step 13** (PCI/ECAM). Prerequisite for networking
(virtio-net, Step 14), AHCI (Step 15), and a future USB/xHCI step (host driver
via the `crab-usb` crate).
**Status:** Design, ready for implementation planning.

## Context

The kernel boots under Limine, parses ACPI with the `acpi` crate in
`acpi_init.rs` (RSDP → MADT → LAPIC/IOAPIC + IRQ overrides), has a bitmap frame
allocator (`memory/frames.rs`), a single `OffsetPageTable` Mapper
(`memory/mapper.rs`) exposing `map_io_page` for uncached MMIO, an APIC + timer +
PS/2 keyboard stack, a VFS, and an `embassy-executor` async runtime.

It has **no PCI subsystem**. Every device driver so far is either legacy
port-mapped (serial `0x3F8`, PS/2 `0x60`) or platform-fixed via ACPI (LAPIC /
IOAPIC MMIO). To drive any PCIe device — first target being an xHCI USB host
controller — the kernel must be able to discover devices on the PCI bus, read
their class/vendor identity, and decode their BARs to find the MMIO window the
device driver will operate on.

On a modern UEFI/`q35` platform (the development target for the USB work) PCIe
configuration space is memory-mapped via ECAM, and its base address(es) are
described by the ACPI **MCFG** table — which the `acpi` crate already exposes
(`acpi::mcfg::Mcfg`) from the `AcpiTables` we already build. This lets us reuse
the existing ACPI bring-up rather than introducing the legacy `0xCF8/0xCFC`
port mechanism.

## Prior art & reuse decision (rev. 2026-05-29)

Surveyed `toku-sa-n/ramen` (Rust hobby OS) for its PCI handling
(`servers/xhci/src/pci/`). Findings: it is **not** directly reusable here —
(1) **GPL-3.0** licensed (copying contaminates our tree), (2) uses the **legacy
`0xCF8/0xCFC` port mechanism** (we chose ECAM), (3) **microkernel** design where
config access is a `syscalls::outl/inl` round-trip (we are monolithic ring-0),
(4) minimal: it only reads a BAR's base (no size probing), has no capability
walk, no command-register helpers, no multifunction probe. Its value is as a
*structural reference* (clean newtypes `Bus`/`Device`/`Function`, 64-bit BAR
combine) and a useful pointer: its xHCI server is built on the **`xhci` crate
0.9.2** (same author, MIT, no_std) + the **`accessor` crate** for volatile MMIO —
both noted for the future xHCI bring-up step, not used here.

**Decision: delegate the config-space *decoding* to the `pci_types` crate**
(rust-osdev, **MIT/Apache-2.0**, `no_std`, pairs with the `acpi` crate we already
build). `pci_types` provides `PciHeader`/`EndpointHeader` (header parse, BAR
decode **with size probing**, 64-bit two-slot handling, capability iterator,
`CommandRegister`/`StatusRegister` with `update_command`). The **only**
kernel-specific code we still write is the `ConfigRegionAccess` trait (two
`unsafe` methods, `read`/`write` `u32` at a `PciAddress`+offset) implemented over
ECAM via `map_io_page` + volatile access. On top of `pci_types` we keep our own
thin API — `PciDevice` cache, `find_class`, `bar(n)`, `enable_mmio`/
`enable_bus_master` — so consumers (xHCI, AHCI) get a stable ruos-shaped surface
("hybrid" approach). This removes hand-rolled BAR sizing, 64-bit slot-skipping,
capability-list walking, and command-bit layout from our codebase.

New crate dependency on the kernel: `pci_types = "0.10"` (track latest; default
features, `no_std`). `acpi`'s MCFG support (`acpi::mcfg`) is already pulled in.

## Goals

- Extract ECAM region(s) from MCFG during the existing `acpi_init::parse()`,
  exposed on `AcpiInfo` as `Vec<EcamRegion>` (mirrors the existing `overrides`
  extraction pattern).
- An `EcamAccess` type implementing `pci_types::ConfigRegionAccess` over the
  ECAM regions, backed by `map_io_page` + volatile `u32` access. This is the
  only kernel-specific config-space code; everything below is `pci_types`.
- A `pci` module that enumerates all functions on every bus covered by the
  ECAM regions and builds a `Vec<PciDevice>` describing each present function:
  vendor/device id, class/subclass/prog-if, header type, decoded BARs — header
  parse and BAR decode (size probing, 32/64-bit, prefetchable) done by
  `pci_types::PciHeader`/`EndpointHeader`, not hand-rolled.
- A lookup API sufficient for the xHCI consumer:
  `pci::find_class(0x0C, 0x03, 0x30)` → controller → `bar(0)` → MMIO window.
- Command-register helpers `enable_mmio()` / `enable_bus_master()` implemented
  via `EndpointHeader::update_command` (Memory Space + Bus Master bits) — both
  required before an xHCI/AHCI controller will DMA.
- Capability enumeration exposed via `pci_types`' `capabilities()` iterator, so
  the *next* spec (MSI/MSI-X) can locate the MSI-X capability for free.
- Boot-time smoke test logged on serial; `make run-test` still asserts
  `TEST_PASS`.

## Non-goals (YAGNI)

- **No legacy `0xCF8/0xCFC` port fallback.** The USB target runs on QEMU `q35`
  (and real UEFI), both of which always provide MCFG. If a `pc`/`i440fx`
  configuration without MCFG is ever needed, the port mechanism is a separate,
  small follow-up (noted under Risks).
- **No PCI-to-PCI bridge recursion.** We enumerate every bus number in each
  MCFG bus range flatly. On `q35` all root-complex devices are directly
  reachable this way. Secondary-bus scanning behind bridges is deferred.
- **No MSI / MSI-X programming.** This spec only *walks* the capability list
  and exposes it. Allocating a vector and writing the MSI-X table is the next
  spec (it depends on this one).
- No hotplug, no PCI power management (D-states), no ASPM, no IOMMU/VT-d, no
  SR-IOV, no config-space caching beyond `map_io_page` idempotency, no
  expansion-ROM handling.

## Architecture

A new sub-tree `kernel/src/pci/`, fed by an MCFG extraction added to the
existing ACPI bring-up, mapped through the existing Mapper, and wired into
`kmain` after paging + ACPI are up:

```
acpi_init::parse()  ── now also extracts MCFG ──▶  AcpiInfo { …, ecam: Vec<EcamRegion> }
                                                          │
                                                          ▼
                                           pci::init(&acpi_info.ecam)
                                                          │
                          ┌───────────────────────────────┼───────────────────────────┐
                          ▼                               ▼                              ▼
                  pci/ecam.rs                       pci/device.rs                   pci/mod.rs
            EcamAccess: impl                  PciDevice (our cache)           public API: init,
            pci_types::ConfigRegionAccess     built via pci_types             device list, find_class,
            (map_io_page, volatile u32)       PciHeader/EndpointHeader        bar(n)/enable_* wrappers
                          │                   (header, BAR+size, caps)
                          │ each function's 4 KiB config space = one map_io_page (UC)
                          ▼
                 memory::mapper::map_io_page(PhysAddr)  ── existing, idempotent ──
                                                          │
                                                          ▼
            consumers: future xHCI bring-up (find_class 0x0C/03/30 → bar(0) →
            hand mapped MMIO base to crab-usb), AHCI (Step 15), future virtio/NIC.
```

## Components

### 1. `acpi_init.rs` — MCFG extraction (extends existing `parse()`)

`acpi 5.1.0` exposes the MCFG table as `acpi::mcfg::Mcfg`, reachable via
`tables.find_table::<Mcfg>()`. `Mcfg::entries()` yields `&[McfgEntry]`, each with
`base_address: u64`, `pci_segment_group: u16`, `bus_number_start: u8`,
`bus_number_end: u8`. The per-function physical address is

```
base_address + ((bus - bus_number_start) << 20 | device << 15 | function << 12)
```

(each function has 4096 bytes of config space). We copy the entries into our own
owned type while `tables` is alive — exactly as `overrides` is built today — so
the `pci` module never depends on the `acpi` crate's lifetimes.

```rust
#[derive(Debug, Copy, Clone)]
pub struct EcamRegion {
    pub segment:    u16,
    pub base:       u64,  // physical base of this segment's ECAM window
    pub bus_start:  u8,
    pub bus_end:    u8,    // inclusive
}
```

Added to `AcpiInfo`:

```rust
pub struct AcpiInfo {
    pub lapic_base:  u64,
    pub ioapic_base: u64,
    pub overrides:   Vec<IrqOverride>,
    pub ecam:        Vec<EcamRegion>,   // NEW (empty Vec if MCFG absent)
    pub hhdm_offset: u64,
}
```

New error variant, following the existing `AcpiInitError` + `Display` style:
`AcpiInitError::NoMcfg`. **Decision:** absence of MCFG is *not* fatal to
`acpi_init::parse()` (other subsystems don't need it); `parse()` returns an
empty `ecam` Vec and lets `pci::init` decide. The `NoMcfg` variant exists for an
explicit logged path if we later want to hard-fail.

### 2. `pci/ecam.rs` — `ConfigRegionAccess` over ECAM

The one kernel-specific piece. `pci_types` addresses config space by
`PciAddress { segment, bus, device, function }` + a `u16` byte offset; we map
that to a physical ECAM address and do a volatile `u32` access.

```rust
pub struct EcamAccess {
    regions: Vec<EcamRegion>,   // copied from AcpiInfo.ecam
    hhdm_offset: u64,
}

impl EcamAccess {
    fn phys(&self, a: PciAddress, offset: u16) -> Option<PhysAddr> {
        let r = self.regions.iter().find(|r|
            r.segment == a.segment() && (r.bus_start..=r.bus_end).contains(&a.bus()))?;
        let f = (u64::from(a.bus() - r.bus_start) << 20)
              | (u64::from(a.device()) << 15)
              | (u64::from(a.function()) << 12);
        Some(PhysAddr::new(r.base + f + u64::from(offset & !0x3)))
    }
}

impl pci_types::ConfigRegionAccess for EcamAccess {
    unsafe fn read(&self, a: PciAddress, offset: u16) -> u32 {
        let phys = self.phys(a, offset).expect("ecam: addr out of range");
        let virt = map_io_page(phys);              // idempotent, UC, returns VirtAddr
        core::ptr::read_volatile(virt.as_ptr::<u32>())
    }
    unsafe fn write(&self, a: PciAddress, offset: u16, value: u32) {
        let phys = self.phys(a, offset).expect("ecam: addr out of range");
        let virt = map_io_page(phys);
        core::ptr::write_volatile(virt.as_mut_ptr::<u32>(), value);
    }
}
```

```
SAFETY notes:
- read/write are core::ptr::{read_volatile,write_volatile}; config space is
  device MMIO, never cached (map_io_page sets NO_CACHE | WRITE_THROUGH).
- map_io_page maps one 4 KiB page per (function, offset>>12) at hhdm_offset+phys,
  same scheme as LAPIC/IOAPIC MMIO; idempotent (AlreadyMapped → Ok), so repeated
  reads of the same function are free. ECAM phys lives in a high MMIO hole
  disjoint from RAM → no virt collision with the RAM HHDM image.
- offset masked to u32 alignment (offset & !0x3); pci_types only issues aligned
  u32 accesses, but we mask defensively.
```

### 3. `pci/device.rs` — `PciDevice` cache built from `pci_types`

We do **not** hand-roll header offsets, BAR sizing, or the capability walk —
`pci_types` does all of it. `PciDevice` is our owned snapshot of one function,
populated once at enumeration so consumers don't need the live `EcamAccess`.

```rust
pub use pci_types::Bar;   // Memory32{address,size,prefetchable} / Memory64{..} / Io{port}

pub struct PciDevice {
    pub address:    PciAddress,   // pci_types (segment/bus/device/function)
    pub vendor_id:  u16,
    pub device_id:  u16,
    pub class:      u8,           // base class
    pub subclass:   u8,
    pub prog_if:    u8,
    pub header_type:u8,
    pub bars:       [Option<Bar>; 6],
}
```

Build (per present function), using a shared `&EcamAccess`:

```rust
let header = PciHeader::new(address);
let (vendor_id, device_id) = header.id(&access);
if vendor_id == 0xFFFF { /* absent */ }
let (_rev, class, subclass, prog_if) = header.revision_and_class(&access);
let header_type = header.header_type(&access);

// Type-0 (endpoint) BARs + size probing + 64-bit slot handling: pci_types.
let mut bars = [None; 6];
if let Some(ep) = EndpointHeader::from_header(header, &access) {
    let mut i = 0;
    while i < 6 {
        bars[i] = ep.bar(i as u8, &access);     // size-probed; None for empty/high-half
        i += matches!(bars[i], Some(Bar::Memory64 { .. })).then(|| 2).unwrap_or(1);
    }
}
```

**Capabilities.** Exposed live (not cached) via `EndpointHeader::capabilities(&access)`
→ `pci_types::CapabilityIterator`, since the next (MSI/MSI-X) spec wants the
typed `PciCapability` (MSI = `Msi`, MSI-X = `MsiX` with BIR + table offset). This
spec only enumerates them; it programs nothing.

### 4. `pci/mod.rs` — public API + enumeration

```rust
pub fn init(ecam: &[EcamRegion], hhdm_offset: u64) -> Result<PciInitInfo, PciError>;
pub fn devices() -> Vec<PciDevice>;   // cloned snapshot (list is tiny, write-once)
pub fn find_class(class: u8, subclass: u8, prog_if: u8) -> Option<PciDevice>;

impl PciDevice {
    pub fn bar(&self, n: usize) -> Option<Bar>;   // cached pci_types::Bar
    pub fn enable_mmio(&self);        // update_command(|c| c.set_memory_space(true))
    pub fn enable_bus_master(&self);  // update_command(|c| c.set_bus_master(true))
    pub fn capabilities(&self) -> impl Iterator<Item = PciCapability>; // re-reads via EcamAccess
}

pub struct PciInitInfo { pub device_count: usize, pub xhci: Option<PciAddress> }

#[derive(Debug)]
pub enum PciError { NoEcam, NotInitialized }
// + impl Display in the existing one-line style.
```

The global `EcamAccess` is stored alongside the device list (a
`spin::Once<(EcamAccess, Mutex<Vec<PciDevice>>)>`) so `enable_*` /
`capabilities()` can re-issue config writes/reads after `init`. `enable_mmio` /
`enable_bus_master` reconstruct an `EndpointHeader` from the cached `PciAddress`
and call `update_command`; `bar(n)` returns the value cached at enumeration.

**Enumeration algorithm.** For each `EcamRegion`, for `bus` in
`bus_start..=bus_end`, for `device` in `0..32`: build `PciHeader` for
`function=0` and read its id via `EcamAccess`; `0xFFFF` ⇒ no device, skip. Else
build the `PciDevice` snapshot for function 0; if `header_type & 0x80`
(multifunction) probe functions `1..8` the same way and snapshot the present
ones. Push every present `PciDevice` into a global
`static PCI: spin::Once<(EcamAccess, Mutex<Vec<PciDevice>>)>`. `find_class` scans
the list.

The global is set once at boot; concurrent mutation isn't a concern (single
producer at init). **Lock order:** `pci::init` calls `map_io_page`, which takes
`MAPPER` then `FRAMES` (existing order). The `PCI` lock is taken only around the
final `Vec` publish and in `devices()`, never while holding `MAPPER`/`FRAMES`,
so it composes with the existing order without a new cycle.

### 5. `main.rs` — `kmain` boot sequence (additions)

`mod pci;` declared alongside the other modules. Wiring goes **after** paging is
up (`memory::init_mapper`) and **after** `acpi_init::parse()` (which now yields
`ecam`), and **before** any PCIe device driver. Concretely, just after the
existing `apic`/`timer`/`keyboard` block and before `vfs::init` is a natural
slot:

```rust
let pci_info = match pci::init(&acpi_info.ecam, acpi_info.hhdm_offset) {
    Ok(i)  => i,
    Err(e) => { kprintln!("ruos: pci init fail: {}", e); halt(); }
};
kprintln!("ruos: pci init ok devices={}", pci_info.device_count);
if let Some(a) = pci_info.xhci {
    kprintln!("ruos: xhci @ {:02x}:{:02x}.{}", a.bus(), a.device(), a.function());
}
```

(`pci::init` records `xhci = find_class(0x0C,0x03,0x30).map(|d| d.address)` for the
log line and the future USB step.)

## Data flow

```
Limine RSDP ─▶ acpi_init::parse ─▶ MCFG entries ─▶ Vec<EcamRegion> (on AcpiInfo)
                                                          │
kmain ─▶ pci::init(&ecam, hhdm):  access = EcamAccess::new(ecam, hhdm)
   for region, bus, device, function:
       PciHeader(addr).id(&access)         # EcamAccess → map_io_page → volatile u32
       if vendor != 0xFFFF:
           PciDevice { ids, class, header_type,
                       bars: EndpointHeader::bar() ×6 }   # pci_types: decode+size+64-bit
           push PciDevice
   publish (access, Vec<PciDevice>) into PCI
   return { device_count, xhci: find_class(0x0C,03,30) }
                                                          │
future xHCI step ─▶ dev = find_class(0x0C,03,30)
                    dev.enable_mmio(); dev.enable_bus_master()
                    Some(Bar::Memory64 { address, size, .. }) = dev.bar(0)
                    map BAR window ─▶ hand virtual base to crab-usb Xhci::new(mmio, kernel)
```

## Error handling

Mirror the existing modules: each error is an enum with a one-line `Display`.
- `acpi_init`: `NoMcfg` (non-fatal; `parse` returns empty `ecam`).
- `pci`: `NoEcam` (init called with empty slice — log and skip cleanly, do not
  panic; a machine with no PCIe is valid), `NotInitialized` (accessor before
  `init`).
- BAR decode/sizing and config reads cannot fail structurally (pci_types over a
  mapped page); a `0xFFFF` vendor is "absent", not an error.
- `map_io_page` errors (`MapError`) propagate as a logged fatal in `init` only
  if they occur on a function we've already confirmed present (a real mapping
  failure there is a kernel bug, not a missing device).

## Testing

`make run-test` stays the gate. The keyboard/timer assertion line is unchanged;
add PCI lines to the captured serial and assert the device count is non-zero
when QEMU is launched with an xHCI controller.

- **QEMU flags:** ensure `-machine q35` (provides MCFG) and add
  `-device qemu-xhci` so a class `0x0C/03/30` function is present. The test
  harness in the `Makefile` (`run-test`) gets these added.
- **Smoke assertions (serial):**
  - `ruos: pci init ok devices=N` with `N >= 1`.
  - `ruos: xhci @ bb:dd.f` present (the controller was found).
  - Log BAR0 of the xHCI function: `ruos: xhci bar0=0x… size=0x…` to prove BAR
    decode + sizing work (xHCI BAR0 is a 64-bit memory BAR).
- **Negative path:** on a `q35` machine *without* `-device qemu-xhci`,
  `find_class` returns `None` and `init` still logs a device count (the q35
  built-in functions) — proves enumeration is independent of the target device.

## Decomposition into tasks

0. **Add `pci_types` dependency** to `kernel/Cargo.toml` (`no_std`); confirm it
   builds on the `x86_64-unknown-none` target with `build-std`.
1. **MCFG extraction** in `acpi_init.rs`: `EcamRegion`, `AcpiInfo.ecam`,
   `NoMcfg` variant; populate from `find_table::<Mcfg>()`. Log region count.
2. **`pci/ecam.rs`**: `EcamAccess` implementing `pci_types::ConfigRegionAccess`
   (phys-addr calc + volatile `u32` read/write over `map_io_page`).
3. **`pci/device.rs`**: `PciDevice` snapshot built from `pci_types`
   `PciHeader`/`EndpointHeader` (ids, class, header type, `bar()` ×6); re-export
   `pci_types::Bar`.
4. **`pci/mod.rs`**: `init` enumeration loop, global `(EcamAccess, Vec<PciDevice>)`,
   `find_class`, `enable_mmio`/`enable_bus_master` (via `update_command`),
   `capabilities`, `PciInitInfo`.
5. **`kmain` wiring** + serial logs.
6. **Makefile**: add `-machine q35 -device qemu-xhci` to the `run`/`run-test`
   QEMU lines; extend the serial assertion.
7. **Docs**: implementation plan under `docs/superpowers/plans/`,
   `README.md` layout/roadmap update, CHANGELOG entries.

## CHANGELOG entries (next sequence after the current highest)

The repo is already past entry 100; take the next free `NN` at implementation
time. Indicative slugs (one per task):
- `NN-26-05-29-pci-ecam-spec-rev.md` (this revision — `pci_types` hybrid)
- `NN-26-05-29-pci-ecam-plan.md`
- `NN-26-05-29-pci-types-dep.md`
- `NN-26-05-29-acpi-mcfg-extract.md`
- `NN-26-05-29-pci-ecam-accessor.md`
- `NN-26-05-29-pci-device-snapshot.md`
- `NN-26-05-29-pci-enumerate-init.md`
- `NN-26-05-29-pci-qemu-test-harness.md`

## Open items for the implementation plan

- **Pin the `pci_types` version** at implementation time and re-check its API:
  `ConfigRegionAccess::{read,write}`, `PciHeader::{id,revision_and_class,
  header_type}`, `EndpointHeader::{from_header,bar,capabilities,update_command}`,
  and the `Bar`/`PciCapability` enums. The snippets here target `0.10`; minor
  versions have moved these signatures before (e.g. `bar()` taking `impl
  ConfigRegionAccess` by value vs by ref).
- Confirm the `acpi` crate's `alloc` feature is on (default in `acpi = "5"`);
  the manual `entries()` copy in §1 relies on it. The manual-copy approach avoids
  borrowing `tables` past `parse()`; do **not** hold an `acpi` `PciConfigRegions`
  across `parse()` for the same lifetime reason.
- **DECIDED:** `devices()` returns a cloned snapshot (list is tiny, write-once),
  not a `MutexGuard` — avoids holding the `PCI` lock across a driver's long init.
- **DECIDED (hybrid):** keep the ruos-shaped API (`PciDevice`/`find_class`/
  `bar`/`enable_*`) over `pci_types`; consumers never see `pci_types` types
  except the re-exported `Bar`/`PciCapability`.
- BAR-window mapping ownership: the driver (xHCI/AHCI step) maps its BAR via a
  small `map_io_range(phys, bytes)` helper added to `mapper.rs` for multi-page
  BARs; the PCI layer stays pure discovery. (Helper is a prereq noted for those
  steps, not built here.)
- MSI-X table location (BIR + Table Offset) comes from `pci_types`'
  `PciCapability::MsiX` in the next spec; confirm that variant carries the BIR +
  offset so the follow-up needs no ad-hoc config reads.
