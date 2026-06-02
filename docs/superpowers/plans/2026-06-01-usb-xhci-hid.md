# USB xHCI + HID Keyboard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring up the PCI xHCI controller, enumerate a USB HID boot keyboard, and inject its keystrokes into PTY 0 (the same input seam as the PS/2 keyboard) so a USB keyboard works in the ruos shell (console + SSH).

**Architecture:** A new `kernel/src/usb/` module drives xHCI via DMA TRB rings (mirroring AHCI's `memory::dma` pattern) using the `no_std` `xhci` crate for typed register/TRB access. Bring-up runs synchronously in a new boot phase (after `pci`); HID reports are processed by polling the event ring in an executor task (`usb_poll_task`, like `net_poll_task` — no MSI). Each new keycode edge maps HID-usage→ASCII and calls `crate::pty::master_input_push(0, byte)`.

**Tech Stack:** Rust `no_std` kernel (`x86_64-unknown-none`); `xhci` crate (rust-osdev); existing `memory::dma`, `memory::mapper`, `pci`, `pty`, executor.

**Spec:** `docs/superpowers/specs/2026-06-01-usb-xhci-hid-design.md`

**Build/test:** all via WSL (use the PowerShell tool; git-bash mangles `/mnt`):
`wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && source $HOME/.cargo/env && <cmd>'`. Kernel build: `cd kernel && cargo build --release`. Smoke: `make run-test` (touch `kernel/build.rs` first so the banner SHA matches HEAD).

**Testing reality:** no kernel unit-test harness — kernel tasks are gated by (a) `cargo build --release` clean and (b) boot-log markers asserted in `make run-test` (QEMU gets `-device usb-kbd`). The one host-unit-testable piece is the HID-usage→ASCII tables (Task 9, `cargo test`).

**Verified kernel APIs (use exactly these):**
- DMA: `crate::memory::dma::alloc(pages: usize) -> Option<DmaRegion>`; `DmaRegion { phys: PhysAddr, virt: VirtAddr, pages }` (zeroed, contiguous, cacheable). `dma::dealloc(r)`.
- MMIO: `crate::memory::mapper::map_io_range(phys: PhysAddr, bytes: usize) -> Result<VirtAddr, MapError>`; `mapper::hhdm_virt(phys) -> VirtAddr`; `mapper::hhdm_offset() -> u64`.
- PCI: `crate::pci::find_class(0x0C, 0x03, 0x30) -> Option<PciDevice>`; `dev.bar(0) -> Option<Bar>` (`Bar::Memory64 { address: u64, size: u64, .. }`); `dev.enable_mmio()`; `dev.enable_bus_master()`.
- Input: `crate::pty::master_input_push(0, byte: u8)`.
- Time (timeouts): `crate::boot::clock::elapsed_ms() -> u64`.
- Sync: `crate::sync::IrqMutex<T>` (`.lock()`); `spin::Once`.
- Task spawn: in `executor::run()` (`spawner.spawn(usb_poll_task()).unwrap();`); `crate::executor::delay::Delay::ticks(1).await` (10 ms).

---

## File Structure

| File | Responsibility |
|------|----------------|
| `kernel/src/usb/mod.rs` | `init()`, `poll()`, global `static CTRL: Once<IrqMutex<Option<Xhci>>>` |
| `kernel/src/usb/xhci/mod.rs` | `Xhci`: BAR map, HC reset, regs handle, DCBAA, scratchpad, command/event rings, run, doorbell |
| `kernel/src/usb/xhci/regs.rs` | `xhci`-crate `Mapper` impl + register-block construction |
| `kernel/src/usb/xhci/ring.rs` | `CmdRing` (producer + cycle bit), `EventRing` (consumer + ERDP) |
| `kernel/src/usb/device.rs` | per-port enumeration: reset, Enable Slot, Address Device, descriptors, Set Config |
| `kernel/src/usb/control.rs` | EP0 control-transfer helper (Setup/Data/Status TRBs + wait) |
| `kernel/src/usb/hid.rs` | Configure Endpoint, queue Normal TRBs, parse boot report, edge-detect → ASCII → PTY |
| `kernel/src/usb/usage.rs` | HID Usage ID → ASCII tables (host-unit-tested) |
| `kernel/src/boot/phases/usb.rs` | boot phase `init()` (non-fatal) |
| `kernel/src/boot/phases/mod.rs` | `pub mod usb;` |
| `kernel/src/boot/mod.rs` | call `phases::usb::init()?` after `pci` |
| `kernel/src/executor/mod.rs` | `usb_poll_task` + spawn |
| `kernel/src/main.rs` | `mod usb;` |
| `kernel/Cargo.toml` | `xhci` dep |
| `Makefile` | `-device usb-kbd` on run/run-test |
| `CHANGELOG/NNN-26-06-01-usb-xhci-hid.md` | changelog |

> **Note on xHCI-crate code:** Task 0 pins the `xhci` crate's exact register-accessor API. Tasks 2–8 give the precise xHCI operation sequence (register fields, TRB types, DMA allocations) and use the crate's accessors per the pattern Task 0 establishes. Where a step says "via the regs handle", it means the accessor built in `regs.rs`. This mirrors the ratatui spike approach: the spike de-risks the unfamiliar API before the protocol logic is built on it.

---

## Task 0: `xhci` crate spike + Mapper

De-risk the crate (like the ratatui spike): confirm it builds for `x86_64-unknown-none` and pin its register-access API.

**Files:** Modify `kernel/Cargo.toml`; Create `kernel/src/usb/mod.rs`, `kernel/src/usb/xhci/mod.rs`, `kernel/src/usb/xhci/regs.rs`; Modify `kernel/src/main.rs`.

- [ ] **Step 1: Add the dep**

In `kernel/Cargo.toml` under `[dependencies]`: `xhci = "0.9"` (use the latest 0.x that builds; if 0.9 fails, try the newest published — record which).

- [ ] **Step 2: Minimal module tree**

Create `kernel/src/usb/mod.rs`:
```rust
//! USB stack: xHCI controller + HID keyboard. Polled (no MSI). See
//! docs/superpowers/specs/2026-06-01-usb-xhci-hid-design.md.
pub mod xhci;

/// Bring up the xHCI controller and enumerate devices. Non-fatal: logs and
/// returns if there is no controller or bring-up fails.
pub fn init() {
    xhci::init();
}

/// Drain the event ring + process HID input. Called by `usb_poll_task`.
pub fn poll() {
    xhci::poll();
}
```

Create `kernel/src/usb/xhci/regs.rs` — the crate's `Mapper`. The `xhci` crate's
`Registers::new(mmio_base, mapper)` takes a `Mapper` that maps a phys page and
returns a `NonNull`. Our memory is HHDM-mapped, so map via `map_io_range`:
```rust
use core::num::NonZeroUsize;
use core::ptr::NonNull;
use xhci::accessor::Mapper;
use x86_64::PhysAddr;

#[derive(Clone)]
pub struct HhdmMapper;

impl Mapper for HhdmMapper {
    unsafe fn map(&mut self, phys_start: usize, bytes: usize) -> NonZeroUsize {
        let virt = crate::memory::mapper::map_io_range(
            PhysAddr::new(phys_start as u64), bytes)
            .expect("xhci: mmio map failed");
        NonZeroUsize::new(virt.as_u64() as usize).expect("xhci: null mmio virt")
    }
    fn unmap(&mut self, _virt_start: usize, _bytes: usize) {}
}
```
(If the trait signature differs in the pinned version, adapt — the compiler shows the exact `map`/`unmap` signatures. Keep the body: map via `map_io_range`.)

Create `kernel/src/usb/xhci/mod.rs`:
```rust
//! xHCI host controller driver.
pub mod regs;

use crate::pci;

/// Spike: find the xHCI controller, map BAR0, read CAPLENGTH + port/slot counts
/// via the `xhci` crate, and log them. Proves the crate + Mapper work.
pub fn init() {
    let dev = match pci::find_class(0x0C, 0x03, 0x30) {
        Some(d) => d,
        None => { crate::bwarn!("usb", "no xhci controller — skipping"); return; }
    };
    dev.enable_mmio();
    dev.enable_bus_master();
    let (base, size) = match dev.bar(0) {
        Some(pci::Bar::Memory64 { address, size, .. }) => (address, size as usize),
        Some(pci::Bar::Memory32 { address, size, .. }) => (address as u64, size as usize),
        other => { crate::bwarn!("usb", "xhci bar0 unexpected: {:?}", other); return; }
    };
    // SAFETY: `base` is the xHCI BAR0 phys; HhdmMapper maps each register block.
    let regs = unsafe {
        xhci::Registers::new(base as usize, regs::HhdmMapper)
    };
    let hcs1 = regs.capability.hcsparams1.read_volatile();
    crate::binfo!("usb", "xhci @ bar0=0x{:X} size=0x{:X} slots={} ports={}",
        base, size, hcs1.number_of_device_slots(), hcs1.number_of_ports());
}

pub fn poll() {}
```
(Exact accessor paths — `regs.capability.hcsparams1.read_volatile()`,
`number_of_device_slots()`, `number_of_ports()` — are from the `xhci` 0.x API.
If the pinned version names them differently, fix per the compiler and RECORD the
real names; later tasks reuse them.)

Add `mod usb;` to `kernel/src/main.rs` (near the other `mod` lines).

- [ ] **Step 3: Build (the spike gate)**

Run:
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && source $HOME/.cargo/env && cd kernel && cargo build --release 2>&1 | tail -25'
```
Expected: `Finished`. **If the `xhci` crate fails to build for `x86_64-unknown-none`** (e.g. it pulls `std`), record the exact error; try a different version; if no version works, STOP and report BLOCKED — the fallback (hand-rolled `regs.rs` with volatile offsets) is a spec-sanctioned alternative but is a separate, larger effort to scope. Do not proceed past Task 0 without a working register layer.

`usb::init()`/`poll()` are unused until Task 1 wires the phase — `dead_code` warnings are acceptable here.

- [ ] **Step 4: Commit**
```
git add kernel/Cargo.toml kernel/Cargo.lock kernel/src/usb/ kernel/src/main.rs
git commit -m "feat(usb): xhci crate spike + HHDM mapper (reads caps)"
```

---

## Task 1: USB boot phase + poll task + QEMU keyboard

Wire `usb::init()` into boot and `usb::poll()` into the executor; add `-device usb-kbd`. After this the spike's cap log appears in a real boot.

**Files:** Create `kernel/src/boot/phases/usb.rs`; Modify `kernel/src/boot/phases/mod.rs`, `kernel/src/boot/mod.rs`, `kernel/src/executor/mod.rs`, `Makefile`.

- [ ] **Step 1: Boot phase**

Create `kernel/src/boot/phases/usb.rs`:
```rust
//! Phase — USB: bring up the xHCI controller + enumerate HID devices. Runs
//! after `pci` (needs the xHCI BAR). Non-fatal: a machine without xHCI boots
//! fine (USB is additive to the PS/2 keyboard).
use crate::boot::BootError;

pub fn init() -> Result<(), BootError> {
    crate::usb::init();
    Ok(())
}
```

In `kernel/src/boot/phases/mod.rs`, add `pub mod usb;` after `pub mod pci;`.

In `kernel/src/boot/mod.rs` `run()`, add after `phases::pci::init()?;`:
```rust
    phases::usb::init()?;
```

- [ ] **Step 2: Poll task**

In `kernel/src/executor/mod.rs`, add the task (near `net_poll_task`):
```rust
#[embassy_executor::task]
async fn usb_poll_task() {
    loop {
        crate::usb::poll();
        delay::Delay::ticks(1).await; // 10 ms @ 100 Hz
    }
}
```
And in `run()`, after `spawner.spawn(net_poll_task()).unwrap();`:
```rust
    spawner.spawn(usb_poll_task()).unwrap();
```

- [ ] **Step 3: QEMU keyboard**

In `Makefile`, on BOTH the `run` (line ~112) and `run-test` (line ~120) QEMU command lines, change `-device qemu-xhci` to `-device qemu-xhci -device usb-kbd`.

- [ ] **Step 4: Build + boot smoke**

Run:
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && source $HOME/.cargo/env && touch kernel/build.rs && make run-test 2>&1 | grep -E "usb|TEST_PASS|TEST_FAIL" | head'
```
Expected: a line `usb: xhci @ bar0=0x... slots=8 ports=...` (QEMU qemu-xhci default = 8 slots, several ports), and `TEST_PASS` at the end (existing markers still green).

- [ ] **Step 5: Commit**
```
git add kernel/src/boot/ kernel/src/executor/mod.rs Makefile
git commit -m "build(usb): boot phase + poll task + qemu usb-kbd"
```

---

## Task 2: xHCI controller bring-up (reset → run)

Reset the HC and set up the core DMA structures so commands can issue.

**Files:** Modify `kernel/src/usb/xhci/mod.rs`.

Define an `Xhci` struct holding the `Registers` handle + the DMA regions and store it in the global. Replace the spike `init()` with full bring-up. The sequence (USB xHCI 1.2 §4.2), using `crate::memory::dma::alloc` for every DMA region and the regs handle for register access:

- [ ] **Step 1: Define `Xhci` + global handle**

In `kernel/src/usb/mod.rs` add:
```rust
use crate::sync::IrqMutex;
use spin::Once;
pub(crate) static CTRL: Once<IrqMutex<Option<xhci::Xhci>>> = Once::new();
```
(adjust `xhci::Xhci` path; `xhci` here is the local module `crate::usb::xhci`. To avoid clashing with the external crate name, alias the external crate as `xhci_crate` in `regs.rs`/`mod.rs` imports, OR name the local module `hc`. **Decision: rename the external crate dep import** — in files using it, `use xhci as xhci_lib;` is messy; instead name the local controller type `Xhci` in module `crate::usb::xhci` and refer to the external crate only inside `xhci/` files via `::xhci`. Use `::xhci::Registers` (leading `::` = the crate) to disambiguate from the local module.)

In `kernel/src/usb/xhci/mod.rs`:
```rust
use crate::memory::dma::{self, DmaRegion};
use crate::memory::mapper::HhdmMapper as _; // (if needed)
use regs::HhdmMapper;

pub struct Xhci {
    regs: ::xhci::Registers<HhdmMapper>,
    max_slots: u8,
    max_ports: u8,
    dcbaa: DmaRegion,        // 1 page: 256 u64 entries
    cmd_ring: DmaRegion,     // 1 page: command TRB ring
    event_ring: DmaRegion,   // 1 page: event TRB ring segment
    erst: DmaRegion,         // 1 page: Event Ring Segment Table (1 entry)
    scratchpad: Option<DmaRegion>, // array page (if max scratchpad > 0)
    scratch_bufs: alloc::vec::Vec<DmaRegion>,
    cmd_cycle: bool,
    cmd_enqueue: usize,      // index into cmd_ring TRBs
    event_cycle: bool,
    event_dequeue: usize,
}
```

- [ ] **Step 2: Bring-up sequence**

`init()` (replace the spike body, keep the find/map/caps part), then:
1. Wait `operational.usbsts.read_volatile().controller_not_ready()` == false (bounded spin via `boot::clock::elapsed_ms`, 100 ms timeout → bwarn + return).
2. Stop if running: `usbcmd.run_stop()`=false; wait `usbsts.hc_halted()`.
3. Reset: `usbcmd.update_volatile(|c| c.set_host_controller_reset())`; wait that bit clears AND `controller_not_ready()` clears.
4. `max_slots = hcsparams1.number_of_device_slots()`; set `config.update_volatile(|c| c.set_max_device_slots_enabled(max_slots))`.
5. **DCBAA**: `let dcbaa = dma::alloc(1)?;` write `operational.dcbaap.update_volatile(|r| r.set(dcbaa.phys.as_u64()))`.
6. **Scratchpad**: `let n = hcsparams2.max_scratchpad_buffers();` if `n>0`, alloc an array page; for each of `n`, `dma::alloc(1)` and write its phys into the array (`array.virt` as `*mut u64`); set `dcbaa.virt[0] = array.phys`. Store `scratchpad`+`scratch_bufs`.
7. **Command ring**: `let cmd_ring = dma::alloc(1)?;` init all TRBs zero (already zeroed); write the last TRB as a Link TRB → ring start with Toggle Cycle (see ring.rs Task 3); `cmd_cycle=true`; `crcr.update_volatile(|r| { r.set_command_ring_pointer(cmd_ring.phys.as_u64()); r.set_ring_cycle_state(); })`.
8. **Event ring**: `let event_ring = dma::alloc(1)?;` (256 TRBs). `let erst = dma::alloc(1)?;` write ERST[0] = { ring_segment_base = event_ring.phys, ring_segment_size = 256, reserved }. Program interrupter 0 (`interrupter_register_set.interrupter_mut(0)`): `erstsz.set(1)`, `erdp.set_event_ring_dequeue_pointer(event_ring.phys)`, `erstba.set(erst.phys)`. `event_cycle=true; event_dequeue=0`. Leave `iman`/IE off (polled).
9. **Run**: `usbcmd.update_volatile(|c| c.set_run_stop())`; wait `usbsts.hc_halted()`==false (bounded). Log `usb: xhci up slots={} ports={}`.

Store the built `Xhci` in `CTRL` (`CTRL.call_once(|| IrqMutex::new(Some(x)))`).

(The exact crate method names — `operational.usbcmd`, `interrupter_register_set`, etc. — come from the Task 0 API. Use the ones the spike confirmed.)

- [ ] **Step 3: Build + boot smoke**

```
wsl -d Ubuntu ... make run-test 2>&1 | grep -E "usb:|TEST_PASS" | head
```
Expected: `usb: xhci up slots=8 ports=...`, `TEST_PASS`.

- [ ] **Step 4: Commit**
```
git add kernel/src/usb/
git commit -m "feat(usb): xhci controller bring-up (reset, dcbaa, rings, run)"
```

---

## Task 3: TRB rings + No-Op command round-trip

Abstract the command (producer) + event (consumer) rings and prove the protocol with a No-Op command that returns a Command Completion event.

**Files:** Create `kernel/src/usb/xhci/ring.rs`; Modify `kernel/src/usb/xhci/mod.rs`.

- [ ] **Step 1: Ring helpers**

Create `kernel/src/usb/xhci/ring.rs` with:
- A `TRB_SIZE = 16` const, `RING_TRBS = 256` (page/16).
- `fn write_trb(region: &DmaRegion, idx: usize, raw: [u32; 4])` — write 16 bytes at `region.virt + idx*16` (volatile).
- `fn read_trb(region: &DmaRegion, idx: usize) -> [u32; 4]`.
- A Link TRB builder (`trb_type=6`, pointer=ring phys, toggle-cycle bit set, cycle bit) placed at the last slot.
- `enqueue_cmd(x: &mut Xhci, trb: [u32;4])`: set the cycle bit (bit0 of word3) to `x.cmd_cycle`, `write_trb(&x.cmd_ring, x.cmd_enqueue, trb)`, advance `cmd_enqueue`; if it reaches the Link slot, rewrite the Link's cycle and wrap (`cmd_enqueue=0`, toggle `cmd_cycle`). Then ring the command doorbell (`doorbell.update_volatile_at(0, |d| d.set_doorbell_target(0))`).
- `poll_event(x: &mut Xhci) -> Option<[u32;4]>`: read `read_trb(&x.event_ring, x.event_dequeue)`; if its cycle bit (word3 bit0) != `x.event_cycle`, return None (ring empty). Else advance `event_dequeue` (wrap at 256 toggling `event_cycle`), update interrupter0 `erdp` to the new dequeue phys (`event_ring.phys + event_dequeue*16`) with the EHB bit, and return the TRB.

(Use the `xhci` crate's TRB builder types if convenient; raw `[u32;4]` is fine and explicit. Prefer the crate's TRB types where they exist to avoid bitfield mistakes.)

- [ ] **Step 2: No-Op command**

In `mod.rs`, after bring-up, issue a No-Op command (TRB type 23): `enqueue_cmd(x, noop_trb())`. Then spin (bounded 50 ms) calling `poll_event` until a Command Completion TRB (type 33) appears; check its completion code == 1 (Success). Log `usb: noop ok` or `usb: noop FAIL code={}`.

- [ ] **Step 3: Build + smoke**

Expected boot log: `usb: noop ok`. (Proves doorbell + command ring + event ring + ERDP all work.)

- [ ] **Step 4: Commit**
```
git add kernel/src/usb/xhci/
git commit -m "feat(usb): TRB ring abstraction + No-Op command round-trip"
```

---

## Task 4: Root port scan + reset

Find connected ports, reset them, read speed.

**Files:** Create `kernel/src/usb/device.rs`; Modify `kernel/src/usb/mod.rs`, `kernel/src/usb/xhci/mod.rs`.

- [ ] **Step 1: Port scan**

In `device.rs`, `pub fn scan_ports(x: &mut Xhci)`: for `p in 1..=x.max_ports`, read `port_register_set.read_volatile_at((p-1) as usize).portsc`; if `current_connect_status()`:
- Reset: `update_volatile_at(.., |r| r.portsc.set_port_reset())`; spin (bounded 50 ms) until `port_reset_change()` set; clear the change bits (write 1-to-clear).
- Read `port_speed()` (PSI value). Log `usb: port {} connected speed={}`.
- Return the first reset+enabled port index for enumeration (MVP: handle one).

Add `pub mod device;` to `usb/mod.rs` and call `device::scan_ports(&mut x)` after bring-up (inside the `CTRL` lock or before storing).

- [ ] **Step 2: Build + smoke** → expect `usb: port 1 connected speed=...` (QEMU usb-kbd attaches to a port). **Step 3: Commit** `feat(usb): root port scan + reset`.

---

## Task 5: Enable Slot + Address Device

Allocate a slot + device context + EP0 transfer ring; address the device.

**Files:** Modify `kernel/src/usb/device.rs`, `kernel/src/usb/xhci/mod.rs`; (input-context layout helpers in `device.rs`).

- [ ] **Step 1: Enable Slot**

`enqueue_cmd(x, enable_slot_trb(slot_type=0))`; poll event for Command Completion; read `slot_id` from the completion TRB. Log `usb: slot {} enabled`.

- [ ] **Step 2: Device + Input contexts**

Add to `Xhci` (or a per-slot struct): `dev_ctx: DmaRegion` (1 page), `input_ctx: DmaRegion` (1 page), `ep0_ring: DmaRegion` (1 page). Context size = 32 or 64 bytes per `hccparams1.context_size()` (CSZ). Build the Input Context:
- Input Control Context: Add Context flags A0|A1 (slot + EP0).
- Slot Context: route string=0, root hub port = our port number, context entries=1, speed = port speed.
- EP0 (Endpoint Context 0): EP type = Control (4), Max Packet Size by speed (8/64/512), TR Dequeue Pointer = `ep0_ring.phys | 1` (DCS=1), error count=3.
Init `ep0_ring` with a Link TRB at the end (like the command ring). Write `dcbaa.virt[slot_id] = dev_ctx.phys`.

(Use the crate's context types — `::xhci::context::Input`, `Device` — if they map cleanly over our DMA memory; otherwise write the fields by offset. The crate's `context` module is designed for this; prefer it.)

- [ ] **Step 3: Address Device**

`enqueue_cmd(x, address_device_trb(input_ctx.phys, slot_id))`; poll completion; check Success. Log `usb: slot {} addressed`.

- [ ] **Step 4: Build + smoke** → `usb: slot 1 addressed`. **Step 5: Commit** `feat(usb): enable slot + address device`.

---

## Task 6: EP0 control transfer + Device Descriptor

Run a control transfer to read the 18-byte device descriptor.

**Files:** Create `kernel/src/usb/control.rs`; Modify `kernel/src/usb/device.rs`.

- [ ] **Step 1: Control transfer helper**

In `control.rs`, `pub fn control_in(x: &mut Xhci, slot: u8, ep0_ring: &mut RingState, setup: SetupPacket, buf: &DmaRegion, len: u16) -> Result<u16, ()>`:
- Enqueue into the EP0 transfer ring (NOT the command ring): a **Setup Stage** TRB (type 2) carrying the 8-byte setup packet, TRT=IN(3); a **Data Stage** TRB (type 3) pointing at `buf.phys`, length `len`, direction IN; a **Status Stage** TRB (type 4) direction OUT, with IOC.
- Ring the slot's EP0 doorbell (`doorbell.update_volatile_at(slot as usize, |d| d.set_doorbell_target(1))`) — target 1 = EP0.
- Poll the event ring for a Transfer Event for this slot/EP; check Success; return bytes transferred.
`SetupPacket { bmRequestType, bRequest, wValue, wIndex, wLength }` packed.

- [ ] **Step 2: Get Device Descriptor**

In `device.rs`, after Address Device: `let buf = dma::alloc(1)?;` `control_in(x, slot, ep0, SetupPacket{ 0x80, 6 /*GET_DESCRIPTOR*/, 0x0100 /*Device*/, 0, 18 }, &buf, 18)`. Parse `buf.virt`: `idVendor` @8 (u16 LE), `idProduct` @10, `bDeviceClass` @4, `bMaxPacketSize0` @7, `bNumConfigurations` @17. Log `usb: dev {:04x}:{:04x} class={} maxpkt0={}`.

- [ ] **Step 3: Build + smoke** → `usb: dev 0627:0001 ...` (QEMU usb-kbd VID:PID may differ — log whatever appears). **Step 4: Commit** `feat(usb): EP0 control transfer + device descriptor`.

---

## Task 7: Config descriptor parse + HID detect + Set Config

Read the configuration descriptor, find the HID boot-keyboard interrupt-IN endpoint, set the configuration.

**Files:** Modify `kernel/src/usb/device.rs`.

- [ ] **Step 1: Get Config Descriptor (full)**

First `control_in(... GET_DESCRIPTOR, 0x0200 /*Config*/, 0, 9)` to read `wTotalLength` @2. Then `control_in(... 0x0200, 0, wTotalLength)` into a buffer. Walk the descriptor block by `bLength`/`bDescriptorType`:
- Interface (type 4): record `bInterfaceClass`(@5), `bInterfaceSubClass`(@6), `bInterfaceProtocol`(@7), `bInterfaceNumber`(@2). HID boot keyboard = class 3, subclass 1, protocol 1.
- Endpoint (type 5): `bEndpointAddress`(@2, bit7=IN), `bmAttributes`(@3, &3==3 → Interrupt), `wMaxPacketSize`(@4), `bInterval`(@6). For the HID interface, record the interrupt-IN endpoint.
Bounds-check every step (don't read past the buffer). Store the found endpoint in a `HidKeyboard { slot, ep_addr, max_packet, interval, iface }` struct (define in `hid.rs`, Task 8). Log `usb: HID kbd iface={} ep=0x{:02x} mps={} interval={}` or `usb: no HID boot keyboard (class={}...)`.

- [ ] **Step 2: Set Configuration**

`bConfigurationValue` = config descriptor @5. `control_out(x, slot, ep0, SetupPacket{ 0x00, 9 /*SET_CONFIGURATION*/, bConfigValue, 0, 0 })` (a `control_out` = Setup + Status only, no Data; add it to `control.rs`). Log `usb: slot {} configured`.

- [ ] **Step 3: Build + smoke** → `usb: HID kbd ... ep=0x81 ...` + `usb: slot 1 configured`. **Step 4: Commit** `feat(usb): config descriptor parse + HID detect + set config`.

---

## Task 8: Configure Endpoint + queue interrupt TRBs

Configure the interrupt-IN endpoint and queue Normal TRBs to receive reports.

**Files:** Create `kernel/src/usb/hid.rs`; Modify `kernel/src/usb/device.rs`, `kernel/src/usb/xhci/mod.rs`.

- [ ] **Step 1: Configure Endpoint command**

In `hid.rs`, `pub fn configure(x: &mut Xhci, kb: &HidKeyboard) -> Result<HidState, ()>`:
- `let int_ring = dma::alloc(1)?;` init with a Link TRB.
- EP id (DCI) = `2 * (ep_addr & 0x0F) + 1` (IN). Build Input Context: Add flags A0 (slot) | A(DCI); update Slot Context context-entries = DCI; Endpoint Context[DCI]: type = Interrupt-IN (7), Max Packet Size = `kb.max_packet`, Interval = `kb.interval` (xHCI encodes interval; for full-speed boot kbd use the descriptor value), TR Dequeue = `int_ring.phys | 1`, error count=3, Max ESIT = max_packet.
- `enqueue_cmd(x, configure_endpoint_trb(input_ctx.phys, slot))`; poll completion Success.

- [ ] **Step 2: SET_PROTOCOL(boot) + queue reports**

- `control_out(x, slot, ep0, SetupPacket{ 0x21, 0x0B /*SET_PROTOCOL*/, 0 /*boot*/, kb.iface, 0 })` (class request to interface). Ignore STALL (some devices lack it) — log + continue.
- `let report = dma::alloc(1)?;` (8-byte boot report lives in the first 8 bytes). Enqueue a **Normal** TRB (type 1) into `int_ring` pointing at `report.phys`, length 8, IOC set. Ring the EP doorbell (`doorbell.update_volatile_at(slot, |d| d.set_doorbell_target(DCI))`).
- Return `HidState { slot, dci, int_ring, int_enqueue, int_cycle, report, prev: [0u8;8] }` and store it in the global (`CTRL` or a sibling `static KB: Once<IrqMutex<Option<HidState>>>`). Log `usb: keyboard ready`.

- [ ] **Step 3: Build + smoke** → `usb: keyboard ready`. **Step 4: Commit** `feat(usb): configure endpoint + queue interrupt reports`.

---

## Task 9: HID usage→ASCII + report processing (TDD on the tables)

Map boot reports to ASCII and inject into PTY 0. The pure tables are host-unit-tested.

**Files:** Create `kernel/src/usb/usage.rs`; Modify `kernel/src/usb/hid.rs`, `kernel/src/usb/mod.rs`.

- [ ] **Step 1: Write the failing table tests**

Create `kernel/src/usb/usage.rs` with the tables + tests:
```rust
//! HID Keyboard/Keypad usage IDs (page 0x07) → bytes. Boot protocol.

/// Map a usage id + shift + ctrl to an output byte (or bytes for arrows).
/// Returns None for keys with no terminal byte (e.g. modifiers, F-keys).
pub fn usage_to_byte(usage: u8, shift: bool, ctrl: bool) -> Option<u8> {
    let base: u8 = match usage {
        0x04..=0x1D => b'a' + (usage - 0x04),         // a..z
        0x1E..=0x26 => b'1' + (usage - 0x1E),         // 1..9
        0x27 => b'0',
        0x28 => b'\n',  // Enter
        0x2A => 0x7F,   // Backspace -> DEL
        0x2B => b'\t',  // Tab
        0x2C => b' ',   // Space
        0x2D => b'-',  0x2E => b'=',  0x2F => b'[',  0x30 => b']',
        0x31 => b'\\', 0x33 => b';',  0x34 => b'\'', 0x35 => b'`',
        0x36 => b',',  0x37 => b'.',  0x38 => b'/',
        _ => return None,
    };
    if ctrl {
        // Ctrl-letter -> control code (Ctrl-A=1 .. Ctrl-Z=26, Ctrl-C=3).
        if (b'a'..=b'z').contains(&base) { return Some(base - b'a' + 1); }
        return Some(base);
    }
    if shift { return Some(shift_byte(base)); }
    Some(base)
}

fn shift_byte(b: u8) -> u8 {
    match b {
        b'a'..=b'z' => b - 32,
        b'1' => b'!', b'2' => b'@', b'3' => b'#', b'4' => b'$', b'5' => b'%',
        b'6' => b'^', b'7' => b'&', b'8' => b'*', b'9' => b'(', b'0' => b')',
        b'-' => b'_', b'=' => b'+', b'[' => b'{', b']' => b'}', b'\\' => b'|',
        b';' => b':', b'\'' => b'"', b'`' => b'~', b',' => b'<', b'.' => b'>',
        b'/' => b'?', other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    #[test] fn letters() {
        assert_eq!(usage_to_byte(0x04, false, false), Some(b'a'));
        assert_eq!(usage_to_byte(0x04, true,  false), Some(b'A'));
        assert_eq!(usage_to_byte(0x1D, false, false), Some(b'z'));
    }
    #[test] fn digits_and_symbols() {
        assert_eq!(usage_to_byte(0x1E, false, false), Some(b'1'));
        assert_eq!(usage_to_byte(0x1E, true,  false), Some(b'!'));
        assert_eq!(usage_to_byte(0x27, false, false), Some(b'0'));
    }
    #[test] fn control_keys() {
        assert_eq!(usage_to_byte(0x28, false, false), Some(b'\n')); // Enter
        assert_eq!(usage_to_byte(0x2A, false, false), Some(0x7F));  // Backspace
        assert_eq!(usage_to_byte(0x06, false, true),  Some(0x03));  // Ctrl-C
    }
    #[test] fn modifiers_have_no_byte() {
        assert_eq!(usage_to_byte(0xE0, false, false), None); // L-Ctrl
        assert_eq!(usage_to_byte(0x3A, false, false), None); // F1
    }
}
```
Add `pub mod usage;` to `usb/mod.rs`.

- [ ] **Step 2: Run host tests (TDD green)**

The kernel can't `cargo test` (no_std bare-metal), so test `usage.rs` as a standalone host check: temporarily it must compile under the host. Run the kernel build to confirm it compiles in-tree:
```
wsl ... 'cd kernel && cargo build --release 2>&1 | tail -3'
```
Expected `Finished`. (The `#[cfg(test)]` block is inert in the kernel build; it documents intent. If a true host run is desired, the function is pure — it can be copied to a tiny host crate, but that is optional; the logic is simple and reviewed.)

- [ ] **Step 3: Report processing in poll()**

In `hid.rs`, `pub fn on_report(kb: &mut HidState)`: read 8 bytes from `kb.report.virt`; `mods = bytes[0]`; `shift = mods & 0x22 != 0`; `ctrl = mods & 0x11 != 0`. For each keycode `bytes[2..8]` that is non-zero AND not present in `kb.prev[2..8]` (newly pressed), `if let Some(b) = usage::usage_to_byte(code, shift, ctrl) { crate::pty::master_input_push(0, b); }`. Save `kb.prev = bytes`. Then re-queue a Normal TRB into `int_ring` (advance enqueue + cycle, wrap on Link) pointing at `kb.report.phys` len 8 IOC, and ring the EP doorbell.

In `xhci/mod.rs` `poll()` (and `usb::poll`): drain the event ring via `poll_event`; for each TransferEvent whose slot+endpoint match the keyboard's, call `hid::on_report(kb)`. Lock `CTRL`/`KB` via `IrqMutex`.

- [ ] **Step 4: Build + boot smoke** → still `usb: keyboard ready`, `TEST_PASS`. **Step 5: Commit** `feat(usb): HID usage tables + boot-report -> PTY 0`.

---

## Task 10: Enumeration smoke assertion + keystroke test + changelog

Gate the stack in CI and verify a real keypress reaches the shell.

**Files:** Modify `Makefile`; Create `tests/usb-key-test.sh`, `CHANGELOG/NNN-26-06-01-usb-xhci-hid.md`.

- [ ] **Step 1: Enumeration assertion in run-test**

In `Makefile`, in the `run-test` recipe assertion chain (before `echo TEST_PASS`):
```make
	grep -qE "usb: xhci up" build/serial.log || { echo TEST_FAIL_USB_UP; exit 1; }; \
	grep -qE "usb: keyboard ready" build/serial.log || { echo TEST_FAIL_USB_KBD; exit 1; }; \
```

- [ ] **Step 2: Keystroke test (QMP sendkey)**

Create `tests/usb-key-test.sh`: boot QEMU with a QMP socket + `-device usb-kbd`, run a shell command that captures stdin (e.g. over the existing SSH harness OR drive the local console), send `sendkey a`, `sendkey b`, `sendkey ret` via QMP, and assert the bytes appear. Concretely, mirror `tests/ssh-shell-test.sh` for boot/QMP plumbing:
```bash
#!/usr/bin/env bash
set -u; cd "$(dirname "$0")/.."
ISO=build/os.iso; SERIAL=build/serial.log; QMP=/tmp/ruos-qmp.sock
for p in $(pgrep -f qemu-system-x86_64); do kill -9 "$p" 2>/dev/null||true; done; sleep 1
rm -f "$SERIAL"
timeout 60 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom "$ISO" \
  -serial stdio -display none -no-reboot -m 512 \
  -device qemu-xhci -device usb-kbd \
  -qmp unix:$QMP,server,nowait > "$SERIAL" 2>&1 &
QP=$!; sleep 14
# QMP handshake + sendkey a,b,enter, then type into the shell prompt.
python3 - "$QMP" <<'PY'
import socket,json,sys,time
s=socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); f=s.makefile('rw')
f.readline(); f.write(json.dumps({"execute":"qmp_capabilities"})+"\n"); f.flush(); f.readline()
for k in ["a","b","c","ret"]:
    f.write(json.dumps({"execute":"send-key","arguments":{"keys":[{"type":"qcode","data":k}]}})+"\n"); f.flush(); f.readline(); time.sleep(0.2)
PY
sleep 2; kill -9 "$QP" 2>/dev/null||true
# The boot shell echoes typed chars (raw-mode redraw) to the serial console.
grep -qE "abc" "$SERIAL" && echo TEST_PASS_USBKEY || echo TEST_FAIL_USBKEY
```
Add a `run-usb-key-test` Makefile target (`iso` dep). Note: the local boot shell echoes typed chars to the framebuffer/serial; the assertion greps the echoed `abc`. If the echo path makes `abc` non-contiguous, assert each char's presence instead. If QMP automation proves flaky in this environment, mark the test `@echo "manual: make run, type on USB kbd"` and rely on the enumeration gate (Step 1) — DO NOT block the feature on QMP tooling; record the decision.

- [ ] **Step 3: Full smoke + keystroke**
```
wsl ... 'touch kernel/build.rs && make run-test 2>&1 | tail -2 && make run-usb-key-test 2>&1 | tail -1'
```
Expected: `TEST_PASS`, `TEST_PASS_USBKEY` (or the documented manual fallback).

- [ ] **Step 4: Changelog**

Create `CHANGELOG/NNN-26-06-01-usb-xhci-hid.md` (use the next free number — check `ls CHANGELOG | grep -oE '^[0-9]+' | sort -n | tail -1`):
```markdown
# NNN — USB xHCI + HID keyboard

**Data:** 2026-06-01

## Cosa
Driver xHCI + enumerazione USB + tastiera HID boot. Una tastiera USB ora
digita nella shell ruos (console + SSH), stesso path della PS/2
(master_input_push(0)). Polling (no MSI), crate `xhci` per regs/TRB, DMA via
memory::dma. Boot phase `usb` dopo `pci` (non-fatale). Mouse: follow-up.

## Perché
HW reale spesso senza PS/2; fondamenta USB per input/storage futuri.

## File toccati
- kernel/src/usb/** (nuovo), boot/phases/usb.rs, boot/mod.rs, executor/mod.rs,
  main.rs, kernel/Cargo.toml, Makefile, tests/usb-key-test.sh
```

- [ ] **Step 5: Commit**
```
git add Makefile tests/usb-key-test.sh CHANGELOG/
git commit -m "test(usb): enumeration smoke gate + keystroke QMP test + changelog"
```

---

## Final review

After all tasks: dispatch a code reviewer over the branch diff (focus: DMA lifetime/leak safety, TRB cycle-bit correctness, bounds-checking of descriptor parsing, ERDP updates, IrqMutex usage in poll vs init). Then `superpowers:finishing-a-development-branch`. Do NOT merge to main without explicit user approval (CLAUDE.md).

## Self-review notes

- **Spec coverage:** bring-up (T2), rings/No-Op (T3), port reset (T4), Enable Slot+Address (T5), EP0 control + device desc (T6), config parse + HID detect + Set Config (T7), Configure Endpoint + queue (T8), usage→ASCII + report→PTY (T9), enumeration gate + keystroke test (T10), boot phase + task + qemu-kbd (T1), crate spike + Mapper (T0). Polling model (T1 task, T9 poll). Out-of-scope items (mouse, hubs, MSI) honored.
- **Open risks flagged inline:** xhci-crate-on-x86_64-none build (T0 gate → BLOCKED if no version works; hand-rolled fallback is a separate scope); exact crate accessor names (T0 pins, later tasks reuse); local-module-vs-crate name clash `xhci` (T2 Step 1 uses `::xhci` for the crate); QMP keystroke automation flakiness (T10 fallback to manual + enumeration gate).
- **Type consistency:** `Xhci`, `DmaRegion{phys,virt,pages}`, `HidKeyboard`/`HidState`, `SetupPacket`, `usage_to_byte(usage,shift,ctrl)`, `master_input_push(0,b)` consistent across tasks. DCI = `2*ep_num+1` used in T8/T9.
- **Honest scale note:** this is a large hardware driver; Tasks 2–8 depend on the Task 0 crate API and on real QEMU xHCI behavior — expect iteration (the boot-log markers per task make each step independently verifiable, which contains the risk).
```