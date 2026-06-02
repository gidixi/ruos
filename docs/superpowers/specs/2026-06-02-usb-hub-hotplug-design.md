# USB hub + hot-plug — Design

**Date:** 2026-06-02
**Status:** draft (design), pending review
**Builds on:** the merged USB xHCI + HID keyboard MVP (`fcbcee4`,
`docs/superpowers/specs/2026-06-01-usb-xhci-hid-design.md`).
**Branch:** `feature/usb-hub-hotplug`.

## Goal

Support USB **hubs** and **runtime hot-plug**: a keyboard plugged in at boot or
at runtime — directly on a root port OR behind one or more hubs — enumerates and
types into the shell; unplugging it (or its hub) tears the device(s) down
cleanly. Verified on QEMU via QMP `device_add`/`device_del` (root and behind a
`usb-hub`).

## Why this is a USB-core rework, not an add-on

The MVP enumerates exactly one device, once, on a root port, in `init()`, storing
it in singleton statics (`DEVICE`/`KBD`/`HID`). Hubs + hot-plug break every one
of those assumptions:

- **Many devices, coming and going** → a slot **registry**, not singletons.
- **Devices behind hubs** → enumeration must address a device *through* its
  parent hub: xHCI slot context needs the **Route String** (the hub-port path)
  and, for full/low-speed devices behind a high-speed hub, the **Transaction
  Translator** fields (parent hub slot id + port). Hubs themselves must be
  flagged in their slot context (Hub bit, number of ports, TT think time).
- **Runtime changes** → the poll loop must react to **Port Status Change Events**
  (root ports) and **hub status-change interrupts** (downstream ports), not just
  Transfer Events.
- **Disconnect** → **Disable Slot** + **free DMA** (the MVP's "leak the scratch
  page" pattern is no longer acceptable when slots churn).

## Architecture — four parts in `kernel/src/usb/`

### 1. Event-dispatch core (`xhci/event.rs` + `ring.rs` refactor)

The single most important change. Every event-ring TRB flows through one
`dispatch(x, ev)`:

- **Transfer Event** (type 32): extract `slot` (word3 bits 24..31) + `dci`
  (word3 bits 16..20). Look the slot up in the registry and route:
  keyboard → `hid::on_report`; hub status-change endpoint → `hub::on_status`;
  otherwise log+ignore.
- **Port Status Change Event** (type 34): root port number = word0 bits 24..31.
  Enqueue a `RootPortChanged(port)` action on the **worklist** (do not enumerate
  inline — see re-entrancy below).
- **Command Completion** (type 33): only a synchronous waiter cares; if none is
  waiting, log.

`wait_for(x, pred)` replaces the MVP's `wait_cmd` / inline control-transfer
spins: it polls the event ring with a bounded deadline, returns the first TRB
matching `pred`, and **routes every non-matching TRB to `dispatch`** instead of
dropping it. This fixes the MVP reviewer's "wait_cmd drops events" hazard *and*
makes nested enumeration safe.

**Re-entrancy / worklist.** Enumeration issues control transfers that complete on
the same event ring `poll()` drains. To avoid unbounded recursion and lost
events: `dispatch` only *enqueues* connect/disconnect **actions**
(`RootPortChanged`, `HubPortChanged{hub_slot, port}`); it never enumerates
inline. `poll()` drains the event ring (dispatching everything), then drains the
**action worklist** iteratively. Processing one action runs a full synchronous
enumeration via `wait_for` (which dispatches foreign transfer events — e.g. a
keyboard report — immediately, and enqueues any further connect actions it
discovers). The worklist drains to empty; recursion depth stays at 1.

### 2. Device registry (`registry.rs`)

```
static SLOTS: IrqMutex<[Option<SlotEntry>; MAX_SLOTS]>   // indexed by slot_id
struct SlotEntry {
    kind: SlotKind,            // Hub | Keyboard | Other
    root_port: u8,
    parent_slot: u8,           // 0 = on a root port
    parent_port: u8,           // hub port this device hangs off (0 if root)
    route: u32,                // route string
    speed: u8,
    dma: DeviceDma,            // ep0_ring, input_ctx, dev_ctx (+ kind-specific)
}
enum SlotKind { Hub(HubState), Keyboard(HidState), Other }
```

`MAX_SLOTS` matches the controller's enabled slots (≤256; bounded array). The
registry owns every DMA region a slot uses, so teardown frees them. Lookups:
by slot_id (event dispatch), by (root_port) and by (parent_slot, parent_port)
(disconnect).

### 3. Enumeration parameterized by location (`device.rs` refactor)

```
struct Location { root_port: u8, route: u32, speed: u8,
                  parent_hub_slot: u8, parent_port: u8, tt: bool }
fn enumerate(x, loc: Location) -> Option<u8 /*slot_id*/>
```

Enable Slot → build slot context (`root_hub_port_number = loc.root_port`,
`route_string = loc.route`, `speed = loc.speed`; if `loc.tt`:
`parent_hub_slot_id`/`parent_port_number` + the device runs through a TT) →
EP0 ring → Address Device → read device + config descriptors. Then dispatch by
class:

- **Hub** (bDeviceClass 0x09): `hub::setup` (below); register `SlotKind::Hub`.
- **HID boot keyboard** (interface 3/1/1): `hid::configure_endpoint` (reused
  from the MVP, unchanged); register `SlotKind::Keyboard`.
- **Other**: log VID:PID/class; register `SlotKind::Other` (no class driver).

The crate exposes every needed slot-context setter (`set_route_string`,
`set_hub`, `set_number_of_ports`, `set_parent_hub_slot_id`,
`set_parent_port_number`, `set_tt_think_time`, `set_multi_tt`,
`set_max_exit_latency`) — verified in `::xhci::context`.

**Route string + TT rules.** Route string is 20 bits = 5 nibbles, one per tier;
nibble `t` = the downstream hub port at tier `t+1`. A root-port device has
`route = 0` (its position is `root_hub_port_number`). A device on hub port `p`
at tier `t` sets `route = parent.route | ((p & 0xF) << (4*t))`. TT: when the
device is full/low-speed and reached through a high-speed hub, set the TT fields
to that hub's slot + the hub port; high-speed/SuperSpeed devices need no TT.
Depth is bounded to 5 tiers (xHCI max); deeper attachments are logged + skipped.

### 4. Hub class driver (`hub.rs`)

`setup(x, slot, dev)` after a hub is addressed + configured:

1. **Hub descriptor** GET_DESCRIPTOR(type 0x29) → `bNbrPorts`,
   `wHubCharacteristics` (TT think time bits, power-switching), `bPwrOn2PwrGood`.
2. **One Configure Endpoint command** (type 12) that does both: with `A0` set it
   carries an updated slot context (Hub bit + `number_of_ports` + `tt_think_time`,
   + `multi_tt` if reported), and with `A(dci)` set it adds the hub's
   **interrupt-IN status-change endpoint** (same endpoint-context build as the
   keyboard's). Allocate that endpoint's interrupt ring here.
3. **Power ports**: SET_FEATURE(PORT_POWER=8) per port; wait
   `bPwrOn2PwrGood * 2` ms.
4. **Queue the status-change TRB**: a Normal TRB into a small change-bitmap
   buffer (`ceil((nports+1)/8)` bytes) on the interrupt ring; ring the doorbell.
5. **Initial scan**: GET_STATUS each port; for each connected port enqueue a
   `HubPortChanged{slot, port}` action so the worklist enumerates it (uniform
   with hot-plug).

`on_status(x, st)` when the hub's interrupt completes: read the change bitmap;
for each set bit `p`, enqueue `HubPortChanged{slot, p}`; re-queue the Normal TRB.

`handle_hub_port(x, hub_slot, port)` (worklist action): GET_STATUS(port). If
newly connected: SET_FEATURE(PORT_RESET=4), poll GET_STATUS for C_PORT_RESET,
read enable + speed, CLEAR_FEATURE the change bits, compute the child `Location`,
`enumerate` it. If disconnected: find the child slot by (hub_slot, port) and tear
it down.

Hub class control requests (over the hub's EP0): GET hub descriptor
(`bmRequestType 0xA0`, GET_DESCRIPTOR, wValue 0x2900), GET_STATUS port
(`0xA3`, request 0, wIndex port, 4 bytes), SET_FEATURE port (`0x23`, request 3),
CLEAR_FEATURE port (`0x23`, request 1). Feature selectors: PORT_RESET=4,
PORT_POWER=8, C_PORT_CONNECTION=16, C_PORT_RESET=20.

## Hot-plug + disconnect

- **Root connect/disconnect**: a Port Status Change Event → `RootPortChanged`.
  On connect (CCS=1, after reset): `enumerate` a root `Location`. On disconnect
  (CCS=0): find the slot on that root port, tear it down.
- **Hub-port connect/disconnect**: via the hub's status-change interrupt →
  `HubPortChanged` → connect enumerates, disconnect tears down.
- **Teardown** (`registry::teardown(x, slot)`): if the slot is a hub, recursively
  tear down every child first (slots whose `parent_slot == slot`). Then issue
  **Disable Slot** (command type 10), clear `DCBAA[slot]`, `dma::dealloc` all the
  slot's DMA regions, and clear the registry entry. A removed keyboard's
  `HidState` is dropped (its `master_input_push` simply stops).
- **Multiple keyboards**: the registry holds N; each `on_report` pushes to PTY 0.

## Polling model

Unchanged cadence: `usb_poll_task` calls `usb::poll()` every ~10 ms. `poll()` now
(1) drains the event ring through `dispatch`, then (2) drains the action
worklist. No MSI; root-port and hub-port changes both surface as ring
events/interrupts, so polling only paces how often we service them.

## Error handling

- Every command/transfer checks completion code; failures `bwarn!` + abort that
  device's enumeration (others continue). A failed child never wedges the hub.
- Bounded waits everywhere (`boot::clock::elapsed_ms`), as in the MVP.
- DMA-alloc failure mid-enumeration: free whatever was allocated for that device,
  log, skip. (Teardown path is reused for partial cleanup.)
- Depth > 5 tiers, unknown speeds, malformed hub/descriptor data: log + skip;
  bounds-check every descriptor walk (as the MVP already does).
- Disconnect during enumeration (device yanked mid-flight): the in-flight control
  transfer times out → enumeration aborts + frees → the subsequent disconnect
  action finds no slot and no-ops. No use-after-free (registry owns DMA).

## Testing

1. **Hot-plug on a root port** (QMP): boot, `device_add usb-kbd`, send keys via
   `send-key`, assert echo; `device_del`, assert no further echo. Exercises Port
   Status Change Events + connect/disconnect teardown.
2. **Keyboard behind a hub at boot**: `-device usb-hub` + `usb-kbd` behind it →
   assert `usb hub … ports=N` + `usb keyboard ready` + a keystroke echoes.
3. **Hot-plug behind a hub** (QMP): with the hub present, `device_add`/`device_del`
   a keyboard on a hub port → keystroke works after add, stops after del.
   Exercises the hub status-change interrupt + route-string/TT addressing.
4. **Regression**: the existing root keyboard (`make run-usb-key-test`) + all
   `make run-test` markers stay green; PS/2 untouched.

New boot-log markers gate the automated parts: `usb hub slot=S ports=N`,
`usb enumerated slot=S kind=… route=0xX`, `usb teardown slot=S`.

## Out of scope (YAGNI)

- Non-HID class drivers (storage, audio, …) — identified + logged, not driven.
- Mouse (no consumer until a GUI exists).
- Power management, suspend/resume, remote wakeup, USB3 streams.
- Multi-TT beyond setting the slot-context bit; per-TT bandwidth scheduling.
- MSI/MSI-X (polling stays).

## Files touched

- `kernel/src/usb/xhci/event.rs` — NEW: `dispatch`, `wait_for`, event decoders.
- `kernel/src/usb/xhci/ring.rs` — refactor: `wait_for`-based waiting; keep
  enqueue/poll primitives.
- `kernel/src/usb/registry.rs` — NEW: `SlotEntry`/`SlotKind`, `SLOTS`, teardown.
- `kernel/src/usb/device.rs` — refactor `address_device` → `enumerate(Location)`;
  route-string/TT slot context; class dispatch.
- `kernel/src/usb/hub.rs` — NEW: hub descriptor, port power/status/reset,
  status-change endpoint, `setup`/`on_status`/`handle_hub_port`.
- `kernel/src/usb/control.rs` — add class/other-recipient request helpers
  (GET_STATUS, SET/CLEAR_FEATURE) over arbitrary slot EP0.
- `kernel/src/usb/hid.rs` — `HidState` moves into the registry; `on_report`
  unchanged logic, looked up by slot.
- `kernel/src/usb/mod.rs` — replace `DEVICE`/`KBD`/`HID` singletons with the
  registry + the worklist; `poll()` = drain events + drain worklist.
- `kernel/src/usb/xhci/mod.rs` — `init()` registers root devices via the worklist;
  `poll()` delegates to the event core.
- `Makefile` — QMP hot-plug + hub test targets; QEMU `usb-hub` topology in tests.
- `tests/usb-hotplug-test.sh`, `tests/usb-hub-test.sh` — NEW.
- `CHANGELOG/NNN-26-06-02-usb-hub-hotplug.md`.
```