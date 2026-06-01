# USB xHCI + HID keyboard — Design

**Date:** 2026-06-01
**Status:** draft (design), pending review
**Scope:** MVP — bring up the xHCI controller, enumerate a single USB HID
**boot keyboard**, and feed its keystrokes into the existing terminal input path
(`pty::master_input_push(0, byte)`), exactly like the PS/2 keyboard. Mouse is a
deliberate follow-up (no consumer exists yet — no GUI).

## Goal

On boot, detect the PCI xHCI controller, reset and start it, reset any connected
port, enumerate the attached device, and if it is a HID boot keyboard, configure
its interrupt-IN endpoint and translate incoming 8-byte boot reports into ASCII
bytes injected into PTY 0. Result: typing on a **USB keyboard** works in the ruos
shell (local console + over SSH, since both read PTY 0's downstream), on QEMU
(`-device qemu-xhci -device usb-kbd`) and on real hardware lacking PS/2.

## Why this is non-trivial

xHCI is a DMA-ring controller. Bring-up requires: mapping BAR0 MMIO, resetting
the host controller, allocating several physically-contiguous DMA structures
(Device Context Base Address Array, command ring, event ring + Event Ring
Segment Table, scratchpad buffer array, per-device input/device contexts,
per-endpoint transfer rings), driving a command/event protocol with cycle-bit
TRB rings, performing USB control transfers (Get Descriptor / Set Address / Set
Configuration) over endpoint 0, then configuring an interrupt endpoint and
polling its transfer ring for HID reports. We mirror the proven DMA pattern from
AHCI/virtio-net and the polling architecture from `net_poll_task` (no MSI).

## Building blocks (verified in the codebase)

- **Input seam:** `crate::pty::master_input_push(0, byte: u8)` — the exact call
  the PS/2 handler uses (`keyboard/mod.rs:190`). USB HID feeds the same.
- **DMA:** `crate::memory::dma::alloc(pages: usize) -> Option<DmaRegion>` where
  `DmaRegion { phys: PhysAddr, virt: VirtAddr, pages }` (zeroed, contiguous).
  Pattern from `ahci/port.rs:157`. Use `.phys` for controller registers, `.virt`
  for CPU access to descriptors/TRBs.
- **MMIO:** `crate::memory::mapper::map_io_range(phys: PhysAddr, bytes: usize)
  -> Result<VirtAddr, MapError>` (uncached, multi-page). For BAR0.
- **PCI:** `crate::pci::find_class(0x0C, 0x03, 0x30) -> Option<PciDevice>`;
  `dev.bar(0)` → `Bar::Memory64 { address, size, .. }`; `dev.enable_mmio()` +
  `dev.enable_bus_master()` (set command bits — confirm exact method names in the
  spike; `pci/device.rs`).
- **Boot phases:** `boot/mod.rs` runs arch → mem → interrupts → **pci** →
  devices → fs → storage → userland. Add a `usb` phase after `pci`.
- **Async task:** `executor::run()` spawns tasks (`net_poll_task`, etc.); add
  `usb_poll_task` (10 ms tick, like net).
- **Register layer:** the **`xhci` crate** (rust-osdev, `no_std`) provides typed
  Capability/Operational/Runtime/Doorbell register accessors + TRB definitions.
  It is parameterised by a `Mapper` trait (maps a register block phys→virt); we
  implement it over `map_io_range`/`hhdm_virt`. A **Task 0 spike** confirms it
  builds for `x86_64-unknown-none` and pins the exact API; fallback = hand-rolled
  volatile register offsets (isolated to `xhci/regs.rs`).

## Architecture — new `kernel/src/usb/` module

```
usb/
  mod.rs        init() (called from the usb boot phase) + poll() (event drain) +
                a global Once<Mutex<Xhci>> controller handle.
  xhci/
    mod.rs      Xhci: BAR map, HC reset, register handles, DCBAA, scratchpad,
                command ring, event ring, run/stop, doorbell ring, slot mgmt.
    regs.rs     `xhci` crate Mapper impl + register-block construction.
    ring.rs     TrbRing: producer ring (command/transfer) with cycle bit +
                enqueue; EventRing: consumer with dequeue + ERDP update.
    trb.rs      TRB helpers (build Link/Normal/Setup/Data/Status/EnableSlot/
                AddressDevice/ConfigureEndpoint; decode CommandCompletion +
                TransferEvent). (May be thin wrappers over the crate's TRB types.)
  device.rs     Enumeration state machine for one port/slot: reset port, Enable
                Slot, build Input Context, Address Device, control transfers
                (Get Device/Config Descriptor, Set Config), descriptor parsing.
  control.rs    Control-transfer helper over EP0 transfer ring (Setup+Data+Status
                TRBs; wait completion via event ring).
  hid.rs        HID boot keyboard: locate interrupt-IN endpoint from the config
                descriptor, Configure Endpoint, queue Normal TRBs into the
                interrupt transfer ring, parse the 8-byte boot report, map HID
                usage IDs → ASCII (modifiers/shift), push to PTY 0.
  usage.rs      HID Usage ID → ASCII tables (unshifted/shifted), like the PS/2
                SCANCODE_MAP but for HID Keyboard/Keypad usage page (0x07).
boot/phases/usb.rs   init(): discover xHCI, bring it up, enumerate ports.
```

## xHCI bring-up sequence (`xhci::mod::init`)

1. `find_class(0x0C,03,30)`; `enable_mmio()` + `enable_bus_master()`.
2. `bar(0)` → phys+size; `map_io_range(phys, size)` → MMIO virt base.
3. Read **Capability** regs: `CAPLENGTH` (op-reg offset), `HCSPARAMS1`
   (MaxSlots, MaxPorts, MaxIntrs), `HCSPARAMS2` (max scratchpad buffers),
   `HCCPARAMS1` (CSZ = context size 32 vs 64 bytes, xECP for legacy handoff),
   `DBOFF` (doorbell array offset), `RTSOFF` (runtime offset).
4. **BIOS→OS handoff** if xECP USBLEGSUP present: set OS-owned bit, wait BIOS
   clears its bit (bounded spin). (QEMU usually no-op; required on real HW.)
5. **Reset**: wait `USBSTS.CNR`=0; `USBCMD.HCRST=1`; wait HCRST clear + CNR=0.
6. Program **MaxSlotsEn** in `CONFIG`.
7. **DCBAA**: alloc 1 page (256×u64), write phys to `DCBAAP`. Slot 0 entry =
   scratchpad-array phys if scratchpad buffers required.
8. **Scratchpad**: if `HCSPARAMS2` max-scratchpad>0, alloc that many 4 KiB
   buffers + a phys-array page; DCBAA[0] = array phys.
9. **Command ring**: alloc 1 page; init `TrbRing` (cycle=1, Link TRB at end);
   write phys|RCS to `CRCR`.
10. **Event ring**: alloc segment (1 page of TRBs) + ERST (1 entry: seg phys +
    size); program interrupter 0 runtime regs: `ERSTSZ`=1, `ERSTBA`=ERST phys,
    `ERDP`=seg phys; `IMAN`/`IMOD` left polled (no IE, no MSI).
11. **Run**: `USBCMD.RS=1`; wait `USBSTS.HCH`=0.

## Port + enumeration (`device.rs`)

For each root port (1..=MaxPorts) read `PORTSC`:
- If `CCS` (connected): write `PORTSC.PR=1` (port reset); wait `PRC`+`PED`; read
  port `Speed`.
- **Enable Slot** command → CommandCompletion gives slot id.
- Alloc **Input Context** (1 page) + **Device Context** (1 page, DCBAA[slot]=its
  phys). Fill Input Control Context (add A0|A1) + Slot Context (root port, speed,
  context entries=1) + EP0 Context (control, MaxPacketSize by speed, TR
  dequeue=EP0 transfer-ring phys|DCS). Alloc EP0 transfer ring (1 page).
- **Address Device** command (input ctx phys, slot) → completion.
- **Control transfers** over EP0 (via `control.rs`):
  - GET_DESCRIPTOR(Device, 18 bytes) → bMaxPacketSize0, idVendor/idProduct,
    bNumConfigurations.
  - GET_DESCRIPTOR(Config, full) → walk interface + endpoint descriptors; detect
    Interface class=0x03 (HID), subclass 0x01 (boot), protocol 0x01 (keyboard);
    record the interrupt-IN endpoint (address, max packet, interval).
  - SET_CONFIGURATION(bConfigurationValue).
- Hand the parsed HID endpoint to `hid.rs`.

(MVP: handle the FIRST connected port with a boot keyboard. Loop logs others.)

## HID keyboard (`hid.rs`)

- **Configure Endpoint**: build Input Context adding the interrupt-IN endpoint
  (EP id = `2*ep_num + 1` for IN; type=Interrupt-IN; MaxPacketSize; Interval from
  descriptor; TR dequeue = a fresh interrupt transfer ring phys|DCS). Issue
  Configure Endpoint command.
- Optionally SET_PROTOCOL(boot=0) on the interface (boot keyboards usually
  default to boot; QEMU usb-kbd reports boot layout — issue it for safety).
- **Queue Normal TRBs**: enqueue one (or a few) Normal TRB pointing at an 8-byte
  DMA report buffer with IOC; ring the endpoint doorbell. Re-queue after each
  completion.
- **poll()** (from `usb_poll_task`): drain the event ring. On a TransferEvent for
  the HID endpoint, read the 8-byte report (`[modifiers, reserved, k0..k5]`),
  diff against the previous report to find newly-pressed usage IDs, map each via
  `usage.rs` (+shift from modifier bit, +Ctrl) to ASCII, and call
  `master_input_push(0, byte)`. Re-queue a Normal TRB. Update ERDP.
- Boot report parsing: a key is "newly pressed" if present in the new keycode
  array but not the previous one (standard rollover-safe edge detection). Modifier
  byte bit0/bit4 = L/R Ctrl, bit1/bit5 = L/R Shift, etc.

## HID Usage → ASCII (`usage.rs`)

Static tables indexed by HID Keyboard usage ID (0x04='a' … 0x1D='z',
0x1E..0x27=digits, plus Enter=0x28→`\n`, Esc=0x29, Backspace=0x2A→0x7F,
Tab=0x2B, Space=0x2C, and the symbol keys). Two tables (base / shifted). Ctrl
maps letters to control codes (e.g. Ctrl-C = 0x03) so `^C` works through the
existing line discipline. Arrow keys → ANSI escape sequences (`\x1b[C` etc.),
matching the PS/2 path's `extended_to_ansi`.

## Polling model

No MSI/MSI-X. `usb_poll_task` calls `usb::poll()` every ~10 ms (1 tick). Bring-up
(`init`) runs at boot synchronously, polling command completions with a bounded
spin loop (timeout → log + abort gracefully, controller left stopped). After
init, all device input flows through the periodic `poll()`. This matches
net/AHCI and keeps the cooperative single-core model intact.

## Integration

- `kernel/src/usb/mod.rs`: `pub fn init()` + `pub fn poll()` + global
  `static USB: Once<IrqMutex<Option<Xhci>>>` (IrqMutex since poll runs in a task
  and init at boot; no ISR touches it in the polled MVP).
- `kernel/src/boot/phases/usb.rs`: `init()` — find xHCI (warn+skip if none),
  bring up, enumerate. Non-fatal (a machine without xHCI boots fine).
- `kernel/src/boot/phases/mod.rs`: `pub mod usb;`. `boot/mod.rs`: call
  `phases::usb::init()?` after `phases::pci::init()?`.
- `kernel/src/executor/mod.rs`: add `spawner.spawn(usb_poll_task()).unwrap();`
  and the task (mirrors `net_poll_task`).
- `kernel/Cargo.toml`: add `xhci = "..."` (pin in spike).
- `Makefile`: add `-device usb-kbd` after `-device qemu-xhci` on the `run` and
  `run-test` QEMU lines (111-112, 119-120).

## Error handling

- No xHCI / no ECAM → warn + skip (non-fatal), like AHCI/net.
- Reset/command timeouts → bounded spin (e.g. 100 ms via `boot::clock::elapsed_ms`
  or a tick budget) → log `bwarn` + abort bring-up, leave HC stopped; the system
  continues with PS/2 only.
- DMA alloc failure → log + skip USB.
- Unsupported device (non-HID, or HID non-keyboard) → log VID/PID/class + skip
  (don't configure). No panic on malformed descriptors (bounds-check every walk).
- Event ring overflow / unexpected TRB → log + advance ERDP (best-effort).

## Testing

1. **Enumeration smoke** (`make run-test`, QEMU adds `-device usb-kbd`): assert
   boot-log markers — `usb: xhci up slots=N ports=M`, `usb: port P reset speed=…`,
   `usb: dev VID:PID class=03/01/01`, `usb: HID keyboard on slot S ep …`,
   `usb: keyboard ready`. This proves the full stack through Set Config +
   Configure Endpoint without needing a keypress.
2. **Keystroke test** (separate, QEMU monitor/QMP): launch with a control socket,
   `sendkey a` / `sendkey ret`, assert the byte reaches the shell (e.g. the smoke
   script runs `cat > /tmp/k` and we verify, or a dedicated QMP harness). If QMP
   automation proves fiddly, document a manual `make run` + type test; the
   enumeration smoke remains the automated gate.
3. **Regression:** existing `make run-test` markers (PS/2 path untouched), and the
   PS/2 keyboard still works (USB is additive — both call `master_input_push(0)`).
4. **VBox / real HW** (later): real USB keyboard; VBox xHCI. Note in the changelog
   that the BIOS→OS handoff path matters on real HW (QEMU skips it).

## Out of scope (YAGNI)

- Mouse (no consumer; follow-up once a GUI exists).
- USB hubs, multiple simultaneous HID devices, hot-unplug re-enumeration beyond
  basic disconnect logging.
- Full HID Report Descriptor parser (boot protocol's fixed 8-byte layout suffices
  for keyboards; report-descriptor parsing is needed only for non-boot devices).
- MSI/MSI-X interrupts (polling is sufficient; can optimise later).
- USB mass storage, isochronous transfers, USB3 streams, power management.

## Files touched

- `kernel/src/usb/{mod,device,control,hid,usage}.rs` — NEW
- `kernel/src/usb/xhci/{mod,regs,ring,trb}.rs` — NEW
- `kernel/src/boot/phases/usb.rs` — NEW; `boot/phases/mod.rs` + `boot/mod.rs` —
  register + call the phase
- `kernel/src/executor/mod.rs` — `usb_poll_task`
- `kernel/src/main.rs` — `mod usb;`
- `kernel/Cargo.toml` — `xhci` crate
- `Makefile` — `-device usb-kbd`
- `CHANGELOG/NNN-26-06-01-usb-xhci-hid.md`
```