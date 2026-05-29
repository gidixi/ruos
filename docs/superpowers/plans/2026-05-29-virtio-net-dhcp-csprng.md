# virtio-net + DHCP + CSPRNG Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Roadmap Step 14 — bring up a real NIC (virtio-net) so the smoltcp stack gets a DHCP lease from QEMU SLIRP, plus a RDRAND-seeded ChaCha20 CSPRNG, without breaking the existing loopback path.

**Architecture:** A reusable contiguous DMA allocator over the bitmap frame allocator backs `virtio_drivers::Hal`. `virtio-drivers` (PCI transport) drives the NIC; the Step 13 `pci` layer discovers it and supplies the ECAM virtual base for the crate's `MmioCam` (device config pages are already HHDM-mapped by PCI enumeration). A `net/virtio.rs` adapter exposes the NIC as a `smoltcp::phy::Device`. `NetState` gains a second Ethernet interface with a DHCPv4 socket, polled alongside loopback by the existing `net_poll_task`. `rng.rs` seeds ChaCha20 from RDRAND and replaces the xorshift `random_get`.

**Tech Stack:** Rust `no_std`, `virtio-drivers = "0.13"`, `rand_chacha` (no_std), `smoltcp 0.11` (+`socket-dhcpv4`), `x86_64`, QEMU `q35` + SLIRP user-net.

**Spec:** `docs/superpowers/specs/2026-05-29-rust-virtio-net-dhcp-csprng-design.md`.

---

## Testing strategy (read first)

Freestanding `no_std` kernel — **no host `cargo test`**. Tests = QEMU boot + grep serial (`make run-test`, 120 s timeout, asserts the shell sentinel + PCI lines). Every "test" here is an integration smoke: a `binfo!` serial line absent before the code exists, present after.

All commands via WSL (`CLAUDE.md`):
```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && <cmd>'
```
Build: `make build`. Logging: `binfo!("tag", ...)` / `bwarn!`. **Branch `feature/step14-net` is already checked out** (the spec was committed there). Do NOT create a branch.

**CHANGELOG rule:** one `CHANGELOG/NN-26-05-29-slug.md` per task. The `NN` shown (124+) are indicative — take the next free number from `CHANGELOG/` at execution time (the repo advances in parallel; numbers may collide). Keep slugs. Commit per task, do NOT push.

**API-drift caution:** `virtio-drivers 0.13` PCI types (`PciRoot`, `MmioCam`, `Cam`, `DeviceFunction`, `Command`, `virtio_device_type`, `PciTransport::new`) and `VirtIONet` method names must be checked against the installed crate source under `~/.cargo/registry/src/.../virtio-drivers-0.13.*/` (read `src/transport/pci/bus.rs`, `src/transport/pci/mod.rs`, `src/device/net.rs`, and `examples/`). The skeletons below are the known shape; adapt names to the actual 0.13 API.

---

## File structure

| File | Responsibility | C/M |
|------|----------------|-----|
| `kernel/Cargo.toml` | deps: virtio-drivers, rand_chacha; smoltcp +socket-dhcpv4 | M |
| `kernel/src/memory/frames.rs` | `allocate_contiguous(n)` (N consecutive free frames) | M |
| `kernel/src/memory/dma.rs` | `DmaRegion`, alloc/dealloc, `KernelHal: virtio_drivers::Hal` | C |
| `kernel/src/memory/mapper.rs` | `map_io_range(phys, bytes)` | M |
| `kernel/src/memory/mod.rs` | re-export dma + map_io_range | M |
| `kernel/src/rng.rs` | RDRAND→ChaCha20 CSPRNG, `fill`/`init` | C |
| `kernel/src/wasm/host/random.rs` | `random_get` uses `rng::fill` | M |
| `kernel/src/pci/mod.rs` | `ecam_virt_base()` accessor | M |
| `kernel/src/net/virtio.rs` | NIC driver + smoltcp Device adapter | C |
| `kernel/src/net/mod.rs` | second iface + DHCP; rename iface/device→_lo | M |
| `kernel/src/main.rs` | `mod rng;` | M |
| `kernel/src/boot/phases/userland.rs` | `rng::init()` before `net::init()` | M |
| `Makefile` | netdev + virtio-net-pci; DHCP assertion | M |

---

## Task 1: Dependencies

**Files:** Modify `kernel/Cargo.toml`.

- [ ] **Step 1: Add deps / extend smoltcp features**

In `[dependencies]`: add
```toml
virtio-drivers = "0.13"
rand_chacha = { version = "0.3", default-features = false }
```
Change the existing `smoltcp` line to add `medium-ethernet` + `socket-dhcpv4` (keep the current features):
```toml
smoltcp = { version = "0.11", default-features = false, features = ["alloc", "medium-ip", "medium-ethernet", "proto-ipv4", "proto-dhcpv4", "socket-tcp", "socket-dhcpv4"] }
```

- [ ] **Step 2: Build (test)**

```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -3'
```
Expected: `Finished`. If `rand_chacha 0.3` pulls `getrandom`/std, pin a version or disable features until it builds `no_std` (it is no_std with `default-features=false`). If `virtio-drivers 0.13` minor differs, accept the resolved `0.13.x`.

- [ ] **Step 3: CHANGELOG + commit**

`CHANGELOG/124-26-05-29-step14-deps.md`:
```markdown
# 124 — Step 14 deps: virtio-drivers + rand_chacha + smoltcp dhcp

**Data:** 2026-05-29

## Cosa
`virtio-drivers 0.13`, `rand_chacha` (no_std); smoltcp += `medium-ethernet` +
`socket-dhcpv4`.

## Perché
Driver virtio-net + CSPRNG ChaCha20 + client DHCPv4 su Ethernet (Step 14).

## File toccati
- kernel/Cargo.toml
```
```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && git add kernel/Cargo.toml kernel/Cargo.lock CHANGELOG/124-26-05-29-step14-deps.md && git commit -m "build(kernel): add virtio-drivers, rand_chacha; smoltcp dhcp features"'
```

---

## Task 2: Contiguous frame allocation + DMA region

**Files:** Modify `kernel/src/memory/frames.rs`; Create `kernel/src/memory/dma.rs`; Modify `kernel/src/memory/mod.rs`.

- [ ] **Step 1: Add `allocate_contiguous` to `Frames` + a public wrapper**

In `frames.rs`, add a method on `impl Frames` (after `allocate_frame`'s impl block — note `allocate_frame` is in the `FrameAllocator` impl; add a normal method in `impl Frames`):

```rust
impl Frames {
    /// Allocate `n` physically-contiguous free frames, returning the first
    /// frame. O(total/64) bitmap scan; marks all `n` used. None if no run fits.
    fn allocate_contiguous(&mut self, n: u64) -> Option<PhysFrame<Size4KiB>> {
        if n == 0 { return None; }
        let mut start: u64 = 0;
        let mut run: u64 = 0;
        let mut f: u64 = 0;
        while f < self.total {
            let (i, b) = Self::idx(f);
            let free = (self.bitmap[i] >> b) & 1 == 0;
            if free {
                if run == 0 { start = f; }
                run += 1;
                if run == n {
                    for g in start..start + n { self.bitmap[(g / 64) as usize] |= 1u64 << (g % 64); }
                    self.used += n;
                    return Some(PhysFrame::containing_address(PhysAddr::new(start * PAGE_SIZE)));
                }
            } else {
                run = 0;
            }
            f += 1;
        }
        None
    }

    fn free_contiguous(&mut self, first: PhysFrame<Size4KiB>, n: u64) {
        let base = first.start_address().as_u64() / PAGE_SIZE;
        for g in base..base + n { self.mark_free(g); }
    }
}
```

Add module-level wrappers next to `allocate_frame`:
```rust
pub fn allocate_contiguous(n: u64) -> Option<PhysFrame<Size4KiB>> {
    FRAMES.lock().as_mut().and_then(|f| f.allocate_contiguous(n))
}

pub fn free_contiguous(first: PhysFrame<Size4KiB>, n: u64) {
    if let Some(f) = FRAMES.lock().as_mut() { f.free_contiguous(first, n); }
}
```

- [ ] **Step 2: Create `kernel/src/memory/dma.rs`**

```rust
//! DMA regions: physically-contiguous frames + their HHDM virtual alias.
//! Reused by virtio (rings/buffers) and by AHCI (Step 15). Ring memory is
//! normal cacheable RAM (x86 is DMA-coherent) — do NOT mark it NO_CACHE.

use x86_64::{PhysAddr, VirtAddr};
use x86_64::structures::paging::{PhysFrame, Size4KiB};

use crate::memory::frames::{allocate_contiguous, free_contiguous, PAGE_SIZE};

#[derive(Debug, Copy, Clone)]
pub struct DmaRegion {
    pub phys:  PhysAddr,
    pub virt:  VirtAddr,
    pub pages: usize,
}

/// Allocate `pages` contiguous frames. The HHDM already maps all RAM, so the
/// virtual alias is `phys + hhdm_offset` (same scheme as `map_io_page`).
pub fn alloc(pages: usize) -> Option<DmaRegion> {
    let first = allocate_contiguous(pages as u64)?;
    let phys = first.start_address();
    let virt = crate::memory::mapper::hhdm_virt(phys);
    // Zero the region (rings must start clean).
    unsafe { core::ptr::write_bytes(virt.as_mut_ptr::<u8>(), 0, pages * PAGE_SIZE as usize); }
    Some(DmaRegion { phys, virt, pages })
}

pub fn dealloc(r: DmaRegion) {
    let first = PhysFrame::<Size4KiB>::containing_address(r.phys);
    free_contiguous(first, r.pages as u64);
}
```

- [ ] **Step 3: Add `hhdm_virt` helper to `mapper.rs`** (dma.rs uses it; also needed by Task 4/Task 6)

In `kernel/src/memory/mapper.rs`, add:
```rust
/// Virtual (HHDM) alias of a physical address. Valid for any RAM/MMIO phys
/// because Limine's HHDM covers all physical memory.
pub fn hhdm_virt(phys: PhysAddr) -> VirtAddr {
    let hhdm = *HHDM_OFFSET.get().expect("mapper: hhdm not initialized");
    VirtAddr::new(phys.as_u64() + hhdm)
}
```

- [ ] **Step 4: Re-export from `memory/mod.rs`**

Add `pub mod dma;` and extend the mapper re-export line to include `map_io_range` (added in Task 3) and `hhdm_virt`:
```rust
pub mod dma;
```
and update the existing `pub use mapper::{...}` to add `hhdm_virt` (and `map_io_range` after Task 3).

- [ ] **Step 5: Build (test)**

```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -3'
```
Expected `Finished` (dead_code warnings for dma until virtio uses it — fine).

- [ ] **Step 6: CHANGELOG + commit**

`CHANGELOG/125-26-05-29-dma-contiguous-frames.md`:
```markdown
# 125 — memory/dma + frame allocazione contigua

**Data:** 2026-05-29

## Cosa
`frames::allocate_contiguous/free_contiguous` (scan bitmap per N frame
consecutivi). `memory/dma.rs`: `DmaRegion` + alloc/dealloc (HHDM alias,
zero-init). `mapper::hhdm_virt`.

## Perché
Le ring/buffer virtio (e AHCI) richiedono memoria DMA fisicamente contigua.

## File toccati
- kernel/src/memory/frames.rs
- kernel/src/memory/dma.rs
- kernel/src/memory/mapper.rs
- kernel/src/memory/mod.rs
```
```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && git add kernel/src/memory/ CHANGELOG/125-26-05-29-dma-contiguous-frames.md && git commit -m "feat(memory): contiguous frame alloc + DMA region helper"'
```

---

## Task 3: `map_io_range`

**Files:** Modify `kernel/src/memory/mapper.rs`, `kernel/src/memory/mod.rs`.

- [ ] **Step 1: Add `map_io_range`** (after `map_io_page` in `mapper.rs`)

```rust
/// Map a multi-page MMIO window: every 4 KiB page covering `[phys, phys+bytes)`
/// is mapped (uncached) via `map_io_page`. Returns the virt of `phys` itself.
/// Idempotent per page.
pub fn map_io_range(phys: PhysAddr, bytes: usize) -> Result<VirtAddr, MapError> {
    let start = phys.as_u64() & !0xFFF;
    let end = (phys.as_u64() + bytes as u64 + 0xFFF) & !0xFFF;
    let mut p = start;
    while p < end {
        map_io_page(PhysAddr::new(p))?;
        p += 0x1000;
    }
    Ok(hhdm_virt(phys))
}
```

- [ ] **Step 2: Re-export** — add `map_io_range` to the `pub use mapper::{...}` list in `memory/mod.rs`.

- [ ] **Step 3: Build (test)**

```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -3'
```
Expected `Finished`.

- [ ] **Step 4: CHANGELOG + commit**

`CHANGELOG/126-26-05-29-map-io-range.md`:
```markdown
# 126 — mapper::map_io_range (BAR multi-pagina)

**Data:** 2026-05-29

## Cosa
`map_io_range(phys, bytes)`: mappa tutte le pagine MMIO del range (uncached),
ritorna il virt di phys. Per le finestre BAR virtio.

## File toccati
- kernel/src/memory/mapper.rs
- kernel/src/memory/mod.rs
```
```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && git add kernel/src/memory/mapper.rs kernel/src/memory/mod.rs CHANGELOG/126-26-05-29-map-io-range.md && git commit -m "feat(memory): map_io_range for multi-page MMIO windows"'
```

---

## Task 4: `KernelHal` (virtio_drivers::Hal)

**Files:** Modify `kernel/src/memory/dma.rs`.

- [ ] **Step 1: Verify the 0.13 Hal trait, then implement it**

Read `~/.cargo/registry/src/*/virtio-drivers-0.13*/src/hal.rs` to confirm the exact `Hal` signature (the known 0.13 shape is below). Append to `dma.rs`:

```rust
use core::ptr::NonNull;
use virtio_drivers::{BufferDirection, Hal, PhysAddr as VPhysAddr};

/// virtio-drivers HAL for ruos: DMA via the contiguous frame allocator, MMIO via
/// map_io_range, identity share/unshare (no IOMMU on our x86 target).
pub struct KernelHal;

unsafe impl Hal for KernelHal {
    fn dma_alloc(pages: usize, _dir: BufferDirection) -> (VPhysAddr, NonNull<u8>) {
        let r = alloc(pages).expect("virtio: dma_alloc out of frames");
        (r.phys.as_u64() as VPhysAddr, NonNull::new(r.virt.as_mut_ptr::<u8>()).unwrap())
    }

    unsafe fn dma_dealloc(paddr: VPhysAddr, _vaddr: NonNull<u8>, pages: usize) -> i32 {
        dealloc(DmaRegion {
            phys:  x86_64::PhysAddr::new(paddr as u64),
            virt:  crate::memory::mapper::hhdm_virt(x86_64::PhysAddr::new(paddr as u64)),
            pages,
        });
        0
    }

    unsafe fn mmio_phys_to_virt(paddr: VPhysAddr, size: usize) -> NonNull<u8> {
        let virt = crate::memory::mapper::map_io_range(
            x86_64::PhysAddr::new(paddr as u64), size,
        ).expect("virtio: mmio map failed");
        NonNull::new(virt.as_mut_ptr::<u8>()).unwrap()
    }

    unsafe fn share(buffer: NonNull<[u8]>, _dir: BufferDirection) -> VPhysAddr {
        // Buffer already lives in HHDM-mapped RAM; phys = virt - hhdm.
        let v = buffer.as_ptr() as *mut u8 as u64;
        let hhdm = crate::memory::mapper::hhdm_offset();
        (v - hhdm) as VPhysAddr
    }

    unsafe fn unshare(_paddr: VPhysAddr, _buffer: NonNull<[u8]>, _dir: BufferDirection) {
        // No bounce buffer / IOMMU: nothing to undo.
    }
}
```

- [ ] **Step 2: Add `hhdm_offset()` accessor to `mapper.rs`** (share() needs virt→phys)

```rust
/// The HHDM offset (phys→virt delta). Panics if paging not initialized.
pub fn hhdm_offset() -> u64 {
    *HHDM_OFFSET.get().expect("mapper: hhdm not initialized")
}
```
Re-export it from `memory/mod.rs`.

- [ ] **Step 3: Build (test)**

```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -3'
```
Expected `Finished`. Fix `BufferDirection`/`PhysAddr` import paths and the trait method signatures to match the actual 0.13 source if the compiler complains (e.g. `virtio_drivers::PhysAddr` may be `usize`; `Hal` may live at `virtio_drivers::Hal`).

- [ ] **Step 4: CHANGELOG + commit**

`CHANGELOG/127-26-05-29-virtio-hal.md`:
```markdown
# 127 — KernelHal per virtio-drivers

**Data:** 2026-05-29

## Cosa
`KernelHal: virtio_drivers::Hal` in `memory/dma.rs`: dma_alloc/dealloc via frame
contigui, mmio_phys_to_virt via map_io_range, share/unshare identity (no IOMMU).
`mapper::hhdm_offset`.

## File toccati
- kernel/src/memory/dma.rs
- kernel/src/memory/mapper.rs
- kernel/src/memory/mod.rs
```
```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && git add kernel/src/memory/ CHANGELOG/127-26-05-29-virtio-hal.md && git commit -m "feat(memory): KernelHal implementing virtio_drivers::Hal"'
```

---

## Task 5: CSPRNG (`rng.rs`) + `random_get` rewire

**Files:** Create `kernel/src/rng.rs`; Modify `kernel/src/main.rs`, `kernel/src/wasm/host/random.rs`, `kernel/src/boot/phases/userland.rs`.

- [ ] **Step 1: Create `kernel/src/rng.rs`**

```rust
//! CSPRNG: ChaCha20 seeded from RDRAND. CLAUDE.md: never seed from the timer.
//! RDRAND absent → fatal (no entropy fallback).

use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::{RngCore, SeedableRng};
use spin::Mutex;

static RNG: Mutex<Option<ChaCha20Rng>> = Mutex::new(None);

fn rdrand_u64() -> u64 {
    use core::arch::x86_64::_rdrand64_step;
    for _ in 0..10 {
        let mut x: u64 = 0;
        // SAFETY: RDRAND availability checked in `init` before any call here.
        if unsafe { _rdrand64_step(&mut x) } == 1 {
            return x;
        }
    }
    panic!("rng: RDRAND failed to produce entropy after 10 retries");
}

fn has_rdrand() -> bool {
    use core::arch::x86_64::__cpuid;
    // CPUID.01H:ECX.RDRAND[bit 30].
    let leaf = unsafe { __cpuid(1) };
    (leaf.ecx >> 30) & 1 == 1
}

pub fn init() {
    if !has_rdrand() {
        panic!("rng: CPU lacks RDRAND — no secure entropy source (CLAUDE.md forbids timer seeding)");
    }
    let mut seed = [0u8; 32];
    for chunk in seed.chunks_mut(8) {
        chunk.copy_from_slice(&rdrand_u64().to_le_bytes());
    }
    *RNG.lock() = Some(ChaCha20Rng::from_seed(seed));
    crate::binfo!("rng", "chacha20 seeded (rdrand)");
}

pub fn fill(buf: &mut [u8]) {
    let mut g = RNG.lock();
    let rng = g.as_mut().expect("rng: not initialized");
    rng.fill_bytes(buf);
}

pub fn next_u64() -> u64 {
    let mut g = RNG.lock();
    g.as_mut().expect("rng: not initialized").next_u64()
}
```

- [ ] **Step 2: Declare `mod rng;` in `main.rs`** (alongside the other `mod` lines).

- [ ] **Step 3: Rewire `random_get`** in `kernel/src/wasm/host/random.rs`

Replace the file's xorshift internals with `crate::rng::fill`. Keep the WASI `link`/`random_get` signature; the body becomes:

```rust
pub fn random_get(
    mut caller: Caller<'_, RuntimeState>,
    buf_ptr: i32,
    buf_len: i32,
) -> Result<i32, Error> {
    let mem = wasm_memory(&caller)?;
    let mut tmp = [0u8; 256];
    let mut remaining = buf_len as usize;
    let mut offset = buf_ptr as usize;
    while remaining > 0 {
        let n = remaining.min(tmp.len());
        crate::rng::fill(&mut tmp[..n]);
        mem.write(&mut caller, offset, &tmp[..n]).map_err(|_| Error::i32_exit(-1))?;
        offset += n;
        remaining -= n;
    }
    Ok(0)
}
```
Remove the now-unused `STATE`/`ensure_seeded`/`next` and the `AtomicU64` import. Keep `link`.

- [ ] **Step 4: Call `rng::init()` in boot** — in `kernel/src/boot/phases/userland.rs`, before `crate::net::init();`:

```rust
    crate::rng::init();
    crate::net::init();
```

- [ ] **Step 5: Build, boot, assert the seed line (test)**

```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test && grep -E "rng .* chacha20 seeded" build/serial.log && echo RNG_OK'
```
Expected: `TEST_PASS` then `RNG_OK`. (QEMU exposes RDRAND on the default CPU. If `has_rdrand()` returns false in your QEMU, add `-cpu host` or `-cpu max` to the QEMU lines — note this in the commit.)

- [ ] **Step 6: CHANGELOG + commit**

`CHANGELOG/128-26-05-29-csprng-rdrand-chacha20.md`:
```markdown
# 128 — CSPRNG RDRAND→ChaCha20; random_get rewire

**Data:** 2026-05-29

## Cosa
`rng.rs`: ChaCha20Rng seedato da RDRAND (CPUID check; fatale se assente),
`fill`/`next_u64`/`init`. `random_get` ora usa `rng::fill`. `rng::init()` a boot
prima di `net::init()`.

## Perché
Entropia sicura per WASI random_get e (futuro) SSH; mai timer come seed.

## File toccati
- kernel/src/rng.rs
- kernel/src/main.rs
- kernel/src/wasm/host/random.rs
- kernel/src/boot/phases/userland.rs
```
```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && git add kernel/src/rng.rs kernel/src/main.rs kernel/src/wasm/host/random.rs kernel/src/boot/phases/userland.rs CHANGELOG/128-26-05-29-csprng-rdrand-chacha20.md && git commit -m "feat(rng): RDRAND-seeded ChaCha20 CSPRNG; rewire random_get"'
```

---

## Task 6: `net/virtio.rs` — driver + smoltcp Device

**Files:** Modify `kernel/src/pci/mod.rs` (ECAM virt base accessor); Create `kernel/src/net/virtio.rs`; Modify `kernel/src/net/mod.rs` (`pub mod virtio;`).

- [ ] **Step 1: Expose the ECAM virtual base from `pci`**

In `kernel/src/pci/mod.rs`, add (the `PciState` already holds `EcamAccess` with the regions; expose the first region's HHDM virt base — virtio-drivers' `MmioCam` does ECAM pointer arithmetic from this base, and the device's config page is already mapped by enumeration):

```rust
/// HHDM virtual base of the first ECAM region, for virtio-drivers' MmioCam.
/// `None` if PCI was not initialized or there is no ECAM region.
pub fn ecam_virt_base() -> Option<usize> {
    let base_phys = PCI.get()?.access.first_base()?;       // EcamAccess::first_base, see below
    Some(crate::memory::mapper::map_io_page(x86_64::PhysAddr::new(base_phys)).ok()?.as_u64() as usize)
}
```
Add to `kernel/src/pci/ecam.rs`, on `impl EcamAccess`, the accessor it uses (the regions are already stored there):
```rust
pub fn first_base(&self) -> Option<u64> { self.regions.first().map(|r| r.base) }
```
`PCI` and its `access` field are defined in `pci/mod.rs` (the `PciState` global from Step 13); adjust the path/field names to match (`PCI.get()?.access`).

- [ ] **Step 2: Create `kernel/src/net/virtio.rs`** (verify 0.13 PCI names against the crate source first)

```rust
//! virtio-net NIC: discovered via the PCI layer, driven by virtio-drivers, and
//! adapted to smoltcp's phy::Device. Polled (no IRQ) by net_poll_task.

use virtio_drivers::device::net::{RxBuffer, VirtIONet};
use virtio_drivers::transport::pci::PciTransport;
use virtio_drivers::transport::pci::bus::{Cam, DeviceFunction, MmioCam, PciRoot, Command};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;

use crate::memory::dma::KernelHal;

const QUEUE_SIZE: usize = 16;
const NET_BUF_LEN: usize = 2048;
const MTU: usize = 1500;

type Inner = VirtIONet<KernelHal, PciTransport, QUEUE_SIZE>;

pub struct VirtioNet {
    inner: Inner,
    mac: [u8; 6],
}

impl VirtioNet {
    /// Find + initialize the virtio-net NIC. None if absent.
    pub fn find_and_init() -> Option<Self> {
        // 1. Discover via our PCI layer (vendor 0x1AF4, class 0x02 network).
        let dev = crate::pci::devices().into_iter()
            .find(|d| d.vendor_id == 0x1AF4 && d.class == 0x02)?;
        dev.enable_mmio();
        dev.enable_bus_master();

        // 2. Build virtio-drivers PciRoot over the ECAM window (HHDM virt base).
        let base = crate::pci::ecam_virt_base()?;
        // SAFETY: base is the mapped ECAM window; MmioCam derefs only the
        // device_function we pass, whose config page is mapped by PCI enum.
        let cam = unsafe { MmioCam::new(base as *mut u8, Cam::Ecam) };
        let mut root = PciRoot::new(cam);

        let df = DeviceFunction {
            bus: dev.address.bus(),
            device: dev.address.device(),
            function: dev.address.function(),
        };
        // Ensure command bits (also set above via our helper; harmless to repeat).
        root.set_command(df, Command::MEMORY_SPACE | Command::BUS_MASTER);

        let transport = PciTransport::new::<KernelHal, _>(&mut root, df).ok()?;
        let inner = Inner::new(transport, NET_BUF_LEN).ok()?;
        let mac = inner.mac_address();
        crate::binfo!("net", "virtio-net mac={:02x?}", mac);
        Some(Self { inner, mac })
    }

    pub fn mac(&self) -> [u8; 6] { self.mac }
}

pub struct VirtioRxToken(RxBuffer);
pub struct VirtioTxToken<'a>(&'a mut Inner);

impl RxToken for VirtioRxToken {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        let buf = self.0;
        let r = f(buf.packet());
        // Recycle handled by Device::receive caller via the inner; see note.
        r
    }
}

impl<'a> TxToken for VirtioTxToken<'a> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut tx = self.0.new_tx_buffer(len);
        let r = f(tx.packet_mut());
        self.0.send(tx).expect("virtio: send failed");
        r
    }
}

impl Device for VirtioNet {
    type RxToken<'a> = VirtioRxToken where Self: 'a;
    type TxToken<'a> = VirtioTxToken<'a> where Self: 'a;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut c = DeviceCapabilities::default();
        c.medium = Medium::Ethernet;
        c.max_transmission_unit = MTU;
        c
    }

    fn receive(&mut self, _ts: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        if !self.inner.can_recv() { return None; }
        let rx = self.inner.receive().ok()?;
        // NOTE: smoltcp's split RxToken/TxToken borrow self separately, which
        // conflicts with virtio-drivers' owned RxBuffer + recycle model. See
        // Step 2b for the recycle reconciliation the implementer must apply.
        Some((VirtioRxToken(rx), VirtioTxToken(&mut self.inner)))
    }

    fn transmit(&mut self, _ts: Instant) -> Option<Self::TxToken<'_>> {
        if !self.inner.can_send() { return None; }
        Some(VirtioTxToken(&mut self.inner))
    }
}
```

> **Step 2b — recycle/borrow reconciliation (REQUIRED, implementer judgment).**
> smoltcp's `receive` returns an `(RxToken, TxToken)` pair that borrow the device,
> but virtio-drivers needs the `RxBuffer` returned via `recycle_rx_buffer` after
> use, and the TxToken also needs `&mut inner` — two mutable borrows. Resolve with
> the standard pattern: have `VirtioRxToken` own the `RxBuffer` and, in its
> `consume`, after `f(buf.packet())`, call back into the NIC to recycle. Since the
> token can't hold `&mut inner` simultaneously with the TxToken, the common fix is
> to make `receive` copy the packet into a heap `Vec<u8>` owned by the RxToken and
> immediately `recycle_rx_buffer` before returning (so neither token holds the
> buffer), and give the TxToken `&mut inner`. Copying one MTU per packet is fine at
> our scale. Implement that copy-and-recycle in `receive`; the RxToken then owns a
> `Vec<u8>`. Adjust `VirtioRxToken` to `VirtioRxToken(alloc::vec::Vec<u8>)`.

- [ ] **Step 3: Declare the module** — add `pub mod virtio;` to `kernel/src/net/mod.rs`.

- [ ] **Step 4: Build (test)**

```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -20'
```
Expected `Finished`. This task is the most likely to need API fixes — reconcile `MmioCam`/`PciRoot`/`DeviceFunction`/`Command`/`PciTransport::new`/`VirtIONet`/`RxBuffer` names and the smoltcp `Device` GAT signatures (`RxToken<'a>`/`TxToken<'a>`) against the installed crate sources. If `Device` is not used yet it will warn dead_code — fine until Task 7.

- [ ] **Step 5: CHANGELOG + commit**

`CHANGELOG/129-26-05-29-virtio-net-driver.md`:
```markdown
# 129 — net/virtio.rs: driver virtio-net + smoltcp Device

**Data:** 2026-05-29

## Cosa
`net/virtio.rs`: discovery via PCI (vendor 0x1AF4/class 0x02), `VirtIONet`
(virtio-drivers, PciTransport su MmioCam ECAM), adapter `smoltcp::phy::Device`
(copy+recycle RX, tx via new_tx_buffer/send). `pci::ecam_virt_base()`.

## File toccati
- kernel/src/net/virtio.rs
- kernel/src/net/mod.rs
- kernel/src/pci/mod.rs
- kernel/src/pci/ecam.rs
```
```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && git add kernel/src/net/ kernel/src/pci/ CHANGELOG/129-26-05-29-virtio-net-driver.md && git commit -m "feat(net): virtio-net driver + smoltcp Device adapter"'
```

---

## Task 7: `net/mod.rs` — second interface + DHCP

**Files:** Modify `kernel/src/net/mod.rs`.

- [ ] **Step 1: Rework `NetState` + `init` + `poll`** (preserve loopback)

Replace `net/mod.rs`'s `NetState`/`init`/`poll` with the dual-interface form. Keep `pub mod loopback; pub mod sockets; pub mod virtio;` and the `NET` static. Verify smoltcp dhcpv4 API names against the installed `smoltcp-0.11` source (`socket::dhcpv4`).

```rust
use smoltcp::iface::{Config, Interface, SocketSet, SocketHandle};
use smoltcp::socket::dhcpv4;
use smoltcp::time::Instant;
use smoltcp::wire::{HardwareAddress, EthernetAddress, IpAddress, IpCidr, Ipv4Address, Ipv4Cidr};

pub struct NetState {
    pub iface_lo:  Interface,
    pub dev_lo:    loopback::Loopback,
    pub iface_net: Option<Interface>,
    pub dev_net:   Option<virtio::VirtioNet>,
    pub sockets:   SocketSet<'static>,
    pub dhcp:      Option<SocketHandle>,
    dhcp_bound:    bool,
}

pub static NET: Mutex<Option<NetState>> = Mutex::new(None);

fn now() -> Instant { Instant::from_millis(crate::timer::ticks() as i64 * 10) }

pub fn init() {
    // Loopback (unchanged behaviour, 127.0.0.1/8).
    let mut dev_lo = loopback::new();
    let mut iface_lo = Interface::new(Config::new(HardwareAddress::Ip), &mut dev_lo, now());
    iface_lo.update_ip_addrs(|a| { a.push(IpCidr::new(IpAddress::v4(127, 0, 0, 1), 8)).unwrap(); });

    let mut sockets = SocketSet::new(alloc::vec::Vec::new());

    // virtio NIC (Ethernet, DHCP) if present.
    let (iface_net, dev_net, dhcp) = match virtio::VirtioNet::find_and_init() {
        Some(mut nic) => {
            let mac = nic.mac();
            let cfg = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
            let iface = Interface::new(cfg, &mut nic, now());
            let dhcp_sock = dhcpv4::Socket::new();
            let handle = sockets.add(dhcp_sock);
            (Some(iface), Some(nic), Some(handle))
        }
        None => {
            crate::bwarn!("net", "no virtio-net (loopback only)");
            (None, None, None)
        }
    };

    *NET.lock() = Some(NetState {
        iface_lo, dev_lo, iface_net, dev_net, sockets, dhcp, dhcp_bound: false,
    });
}

pub fn poll() {
    use x86_64::instructions::interrupts::without_interrupts;
    without_interrupts(|| {
        let mut g = NET.lock();
        let Some(net) = g.as_mut() else { return; };
        let t = now();
        let _ = net.iface_lo.poll(t, &mut net.dev_lo, &mut net.sockets);
        if let (Some(iface), Some(dev)) = (net.iface_net.as_mut(), net.dev_net.as_mut()) {
            let _ = iface.poll(t, dev, &mut net.sockets);
            if let Some(h) = net.dhcp {
                let sock = net.sockets.get_mut::<dhcpv4::Socket>(h);
                match sock.poll() {
                    Some(dhcpv4::Event::Configured(cfg)) => {
                        iface.update_ip_addrs(|a| {
                            a.clear();
                            a.push(IpCidr::Ipv4(cfg.address)).unwrap();
                        });
                        if let Some(router) = cfg.router {
                            let _ = iface.routes_mut().add_default_ipv4_route(router);
                        }
                        if !net.dhcp_bound {
                            net.dhcp_bound = true;
                            let gw = cfg.router.map(|r| r).unwrap_or(Ipv4Address::UNSPECIFIED);
                            crate::binfo!("net", "dhcp bound ip={} gw={}", cfg.address.address(), gw);
                        }
                    }
                    Some(dhcpv4::Event::Deconfigured) => {
                        iface.update_ip_addrs(|a| a.clear());
                        let _ = iface.routes_mut().remove_default_ipv4_route();
                        net.dhcp_bound = false;
                    }
                    None => {}
                }
            }
        }
    });
}
```

> Note: existing socket demos bind `127.0.0.1` → serviced by `iface_lo`. The
> sockets pool (`net/sockets.rs`) is unchanged. If `sockets.rs` or other code
> referenced `net.iface`/`net.device` by those old names, update them to
> `iface_lo`/`dev_lo` (grep first: `grep -rn "\.iface\b\|\.device\b" kernel/src/net kernel/src/wasm`).

- [ ] **Step 2: Build, boot, assert the DHCP lease (the headline test)**

```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make build 2>&1 | tail -5'
```
(Full run-test happens in Task 8 once the QEMU netdev is added; here just confirm it compiles. If you want an early check, you can temporarily add the netdev flags manually.)
Expected `Finished`.

- [ ] **Step 3: CHANGELOG + commit**

`CHANGELOG/130-26-05-29-net-dhcp-dual-iface.md`:
```markdown
# 130 — net: seconda interfaccia Ethernet + DHCPv4

**Data:** 2026-05-29

## Cosa
`NetState` con loopback (127/8, invariato) + interfaccia virtio Ethernet
opzionale; socket dhcpv4; `poll()` polla entrambe e applica il lease
(IP + default route), logga `net: dhcp bound ip=.. gw=..`. Rinominati
iface/device → iface_lo/dev_lo.

## File toccati
- kernel/src/net/mod.rs
```
```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && git add kernel/src/net/mod.rs CHANGELOG/130-26-05-29-net-dhcp-dual-iface.md && git commit -m "feat(net): second Ethernet interface + DHCPv4 client"'
```

---

## Task 8: QEMU netdev + run-test DHCP assertion

**Files:** Modify `Makefile`.

- [ ] **Step 1: Add the netdev + NIC to `run` and `run-test`**

Read the current `Makefile` `run`/`run-test` recipes (they already have `-machine q35 -device qemu-xhci` and `run-test` has timeout 120 + shell/PCI greps). Add `-netdev user,id=net0 -device virtio-net-pci,netdev=net0` to BOTH qemu command lines. In `run-test`, add a DHCP assertion after the existing ones:

```makefile
	grep -qE "net .* dhcp bound ip=10\.0\.2\.15" build/serial.log || { echo TEST_FAIL_DHCP; exit 1; }
```
(Place it alongside the shell/PCI/XHCI grep gates; keep `echo TEST_PASS` last.)

- [ ] **Step 2: Run the full gate (the headline test)**

```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -6'
```
Expected: `TEST_PASS`. The serial log must contain `net: dhcp bound ip=10.0.2.15 gw=10.0.2.2`. If DHCP never binds: check `net: virtio-net mac=..` appears (driver found the NIC) and `rng: chacha20 seeded`; if the NIC isn't found, verify the `-device virtio-net-pci` is on the q35 PCIe bus and that `pci init ok devices=` increased. Debugging tip: temporarily `binfo!` the virtio init path.

- [ ] **Step 3: CHANGELOG + commit**

`CHANGELOG/131-26-05-29-qemu-netdev-dhcp-gate.md`:
```markdown
# 131 — QEMU netdev virtio-net + gate DHCP

**Data:** 2026-05-29

## Cosa
`run`/`run-test`: `-netdev user,id=net0 -device virtio-net-pci,netdev=net0`.
`run-test` asserisce `net: dhcp bound ip=10.0.2.15`.

## File toccati
- Makefile
```
```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && git add Makefile CHANGELOG/131-26-05-29-qemu-netdev-dhcp-gate.md && git commit -m "test(qemu): virtio-net netdev + assert DHCP lease"'
```

---

## Task 9 (OPTIONAL): `/dev/random` backed by CSPRNG

Only do this if Task 5 landed cleanly and time allows. The primary CSPRNG consumer is `random_get`; `/dev/random` is a convenience.

**Files:** Modify `kernel/src/vfs/devices.rs`, `kernel/src/vfs/file.rs`, `kernel/src/vfs/tmpfs.rs`.

- [ ] **Step 1: Add `RandomFile`** in `vfs/devices.rs` (mirror `ZeroFile`, fill via rng):

```rust
pub struct RandomFile;

impl File for RandomFile {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, VfsError> {
        crate::rng::fill(buf);
        Ok(buf.len())
    }
    async fn write(&mut self, buf: &[u8]) -> Result<usize, VfsError> { Ok(buf.len()) }
    async fn seek(&mut self, _off: i64, _w: Whence) -> Result<u64, VfsError> { Ok(0) }
    async fn stat(&self) -> Result<crate::vfs::fs::VfsStat, VfsError> {
        Ok(crate::vfs::fs::VfsStat { kind: crate::vfs::fs::VfsKind::Device, size: 0 })
    }
}
```

- [ ] **Step 2: Wire it through `FileImpl` + `TmpKind`** — in `vfs/file.rs` add `Random(RandomFile)` to the `FileImpl` enum (and its match arms for read/write/seek/stat, mirroring `Zero`); in `vfs/tmpfs.rs` add a `TmpKind::DevRandom` variant, the two `=> Ok(FileImpl::Random(RandomFile))` arms (mirroring `DevZero` at the two sites), and create the `/dev/random` node where the dev nodes are populated (mirror `/dev/zero`). Grep the dev-node creation: `grep -rn "DevZero\|/dev/zero" kernel/src/vfs`.

- [ ] **Step 3: Build + smoke**

```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -3'
```
Expected `TEST_PASS` (no regression). Optionally verify `/dev/random` readable via a wasm tool if one exists.

- [ ] **Step 4: CHANGELOG + commit**

`CHANGELOG/132-26-05-29-dev-random.md` (adjust number):
```markdown
# 132 — /dev/random backed da CSPRNG

**Data:** 2026-05-29

## Cosa
`RandomFile` (read → `rng::fill`) wirato come `/dev/random` (FileImpl::Random,
TmpKind::DevRandom).

## File toccati
- kernel/src/vfs/devices.rs
- kernel/src/vfs/file.rs
- kernel/src/vfs/tmpfs.rs
```
```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && git add kernel/src/vfs/ CHANGELOG/132-26-05-29-dev-random.md && git commit -m "feat(vfs): /dev/random backed by CSPRNG"'
```

---

## Task 10: Roadmap Step 14 → DONE

**Files:** Modify `docs/superpowers/roadmap-rust-os.md`.

- [ ] **Step 1: Mark done** — change the Step 14 header to `## Step 14 — Networking (✅ DONE)`.

- [ ] **Step 2: CHANGELOG + commit**

`CHANGELOG/133-26-05-29-roadmap-step14-done.md` (adjust number):
```markdown
# 133 — Roadmap: Step 14 Networking completato

**Data:** 2026-05-29

## Cosa
Step 14 (Networking) ✅ DONE: virtio-net + DHCP lease verificato, CSPRNG attivo.

## File toccati
- docs/superpowers/roadmap-rust-os.md
```
```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && git add docs/superpowers/roadmap-rust-os.md CHANGELOG/133-26-05-29-roadmap-step14-done.md && git commit -m "docs(roadmap): mark Step 14 Networking done"'
```

---

## Done criteria

- `make run-test` → `TEST_PASS` with serial containing `net: dhcp bound ip=10.0.2.15 gw=10.0.2.2`, `net: virtio-net mac=..`, `rng: chacha20 seeded (rdrand)`, plus the unchanged shell + PCI gates.
- Loopback `127.0.0.1` socket demos still work (non-regression).
- `random_get` is RDRAND/ChaCha20-backed (no xorshift left).
- DMA allocator + `map_io_range` are in place for AHCI (Step 15) to reuse.

## Notes for the implementer

- **virtio-drivers 0.13 is the highest-risk surface.** Before Task 6, read the
  crate's `examples/` and `src/transport/pci/` to fix exact names
  (`MmioCam::new`, `PciRoot::new`, `DeviceFunction` fields, `Command` flags,
  `PciTransport::new::<H,_>`, `VirtIONet` methods, `RxBuffer::packet`). If the
  PCI enumeration boundary is awkward, the fallback is to let `PciRoot`
  enumerate the bus itself (`root.enumerate_bus(0)` + `virtio_device_type`)
  instead of our `pci::devices()` — but prefer reusing our discovery.
- **smoltcp 0.11 dhcpv4**: confirm `dhcpv4::Socket::new()` (it may take no args or
  require feature-gated config) and `Event::Configured(cfg)` field names
  (`cfg.address: Ipv4Cidr`, `cfg.router: Option<Ipv4Address>`).
- **`-cpu` for RDRAND**: if QEMU's default CPU lacks RDRAND, add `-cpu max` to
  both QEMU lines (and note it in Task 5/8 commits).
- **Two interfaces, one SocketSet**: polling both `iface_lo` and `iface_net`
  against the shared `sockets` each tick is correct in smoltcp's model.
```
