# USB hub + hot-plug Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax. **Parallelism:** tasks tagged `[PAR]` are independent (pure functions / scripts / research) and may run as concurrent agents up front; tasks tagged `[SEQ]` form the coupled USB core and MUST run in order (each builds on the prior, same files — parallel = conflicts).

**Goal:** USB hubs + runtime hot-plug: a keyboard at boot or runtime, on a root port or behind hub(s), enumerates and types into the shell; unplugging tears it down. Verified on QEMU via QMP.

**Architecture:** Rework the MVP's single-device/singleton model into a USB core: a slot **registry**, a central **event-dispatch** (`dispatch`/`wait_for`) that routes every event-ring TRB, a **worklist** of connect/disconnect actions drained by `poll()`, location-parameterized **enumeration** (route string + single-TT), and a **hub class driver** (descriptor, port power/status/reset, status-change interrupt, recursive enumeration). Polled (no MSI).

**Tech Stack:** Rust `no_std`; `xhci` 0.9.2 crate; existing `memory::dma`, `pty`, executor. Builds on merged MVP (`fcbcee4`).

**Spec:** `docs/superpowers/specs/2026-06-02-usb-hub-hotplug-design.md`

**Build/test (WSL via PowerShell tool; git-bash mangles /mnt):**
`wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && source $HOME/.cargo/env && <cmd>'`. Build: `cd kernel && cargo build --release 2>&1 | tail -20`. Smoke: `touch kernel/build.rs && make run-test`. Kill stray qemu if disk locked: `for p in $(pgrep -f qemu-system); do kill -9 $p; done`. Boot-log tag is `usb ` (space), not `usb:`.

**xHCI command types:** Enable Slot 9, Disable Slot 10, Address Device 11, Configure Endpoint 12, Evaluate Context 13, No-Op 23. **Event TRB types:** Transfer 32, Command Completion 33, Port Status Change 34.

**Hub class constants:** hub descriptor type 0x29 (GET via bmRequestType 0xA0, wValue 0x2900); port GET_STATUS bmRequestType 0xA3 request 0 wIndex=port len 4 (u16 wPortStatus + u16 wPortChange); SET_FEATURE 0x23 request 3; CLEAR_FEATURE 0x23 request 1. Feature selectors: PORT_RESET 4, PORT_POWER 8, C_PORT_CONNECTION 16, C_PORT_RESET 20. wPortStatus bits: 0=connect, 1=enable, 4=reset, 8=power, 9=low-speed, 10=high-speed. wPortChange bits: 0=connect-change, 4=reset-change. Hub desc bytes: [2]=bNbrPorts, [3..4]=wHubCharacteristics (bits5..6=TT think time), [5]=bPwrOn2PwrGood (×2 ms).

**Existing reusable APIs:** `dma::alloc(pages)->DmaRegion{phys,virt,pages}` + `dma::dealloc(r)`; `ring::{enqueue_cmd(x,words,type), enqueue_xfer(ring,&mut enq,&mut cyc,words), poll_event(x)->Option<[u32;4]>, trb_type(&w), completion_code(&w), init_link(virt,phys,cyc)}`; `control::{Setup, control_in(x,dev,s,buf)->Option<u16>, control_out(x,dev,s)->bool}`; `hid::{HidKeyboard, HidState, configure_endpoint(x,dev,kb)->Option<HidState>, on_report(x,st)}`; `boot::clock::elapsed_ms()`; `pty::master_input_push(0,b)`; `sync::IrqMutex`; `crate::binfo!/bwarn!`.

---

## File Structure

| File | Responsibility | Tag |
|------|----------------|-----|
| `kernel/src/usb/encoding.rs` | NEW pure: route string, TT need, speed→maxpkt, hub-desc + port-status decoders (+host tests) | PAR |
| `tests/usb-hotplug-test.sh`, `tests/usb-hub-test.sh` | NEW QMP/QEMU test scripts | PAR |
| `kernel/src/usb/registry.rs` | NEW SlotEntry/SlotKind/SLOTS + worklist + teardown | SEQ |
| `kernel/src/usb/xhci/event.rs` | NEW dispatch + wait_for | SEQ |
| `kernel/src/usb/xhci/ring.rs` | refactor wait_cmd → wait_for-based | SEQ |
| `kernel/src/usb/control.rs` | refactor waits → wait_for; add hub class requests | SEQ |
| `kernel/src/usb/device.rs` | address_device → `enumerate(Location)` + class dispatch + register | SEQ |
| `kernel/src/usb/hub.rs` | NEW hub class driver | SEQ |
| `kernel/src/usb/hid.rs` | HidState into registry; on_report looked up by slot | SEQ |
| `kernel/src/usb/mod.rs` | drop singletons; registry + worklist; poll = events+worklist | SEQ |
| `kernel/src/usb/xhci/mod.rs` | init() seeds root ports via worklist; poll delegates to event core | SEQ |
| `Makefile`, CHANGELOG | targets + entry | SEQ(final) |

---

## Task 0 [PAR]: Pure encoding/decoders + host tests

Independent pure functions consumed by enumerate/hub. No in-tree deps → build first, in parallel.

**Files:** Create `kernel/src/usb/encoding.rs`; modify `kernel/src/usb/mod.rs` (`pub mod encoding;`).

- [ ] **Step 1: Write encoding.rs with functions + `#[cfg(test)]` tests**

```rust
//! Pure USB topology/descriptor helpers (no hardware access) — unit-tested.

/// Route string for a device on `hub_port` of a hub whose own route is
/// `hub_route` and whose tier is `hub_tier` (root-attached hub = tier 0).
/// xHCI route string = 5 nibbles; nibble `hub_tier` holds this hop's port.
pub fn child_route(hub_route: u32, hub_port: u8, hub_tier: u8) -> u32 {
    hub_route | (((hub_port as u32) & 0xF) << (4 * hub_tier as u32))
}

/// Max tier depth supported (route string is 5 nibbles = 20 bits).
pub const MAX_TIER: u8 = 5;

/// A full/low-speed device reached through a high-speed hub needs a TT.
/// speeds: 1=Full, 2=Low, 3=High, 4=Super (xHCI PSI / hub-status mapping).
pub fn needs_tt(child_speed: u8, hub_speed: u8) -> bool {
    matches!(child_speed, 1 | 2) && hub_speed == 3
}

/// Control-endpoint max packet size by speed.
pub fn max_packet0(speed: u8) -> u16 { match speed { 4 => 512, 3 => 64, _ => 8 } }

/// Decode a USB 2.0 hub port status (wPortStatus, wPortChange) → fields.
pub struct PortStatus { pub connected: bool, pub enabled: bool, pub reset: bool, pub speed: u8 }
pub fn decode_port_status(wstatus: u16, _wchange: u16) -> PortStatus {
    let speed = if wstatus & (1<<9) != 0 { 2 }       // low
        else if wstatus & (1<<10) != 0 { 3 }         // high
        else { 1 };                                   // full
    PortStatus {
        connected: wstatus & 1 != 0,
        enabled:   wstatus & (1<<1) != 0,
        reset:     wstatus & (1<<4) != 0,
        speed,
    }
}

/// Hub descriptor key fields.
pub struct HubDesc { pub nbr_ports: u8, pub tt_think_time: u8, pub pwr_on_2_pwr_good_ms: u16 }
pub fn decode_hub_desc(d: &[u8]) -> Option<HubDesc> {
    if d.len() < 6 { return None; }
    let wch = (d[3] as u16) | ((d[4] as u16) << 8);
    Some(HubDesc {
        nbr_ports: d[2],
        tt_think_time: ((wch >> 5) & 0x3) as u8,
        pwr_on_2_pwr_good_ms: (d[5] as u16) * 2,
    })
}

#[cfg(test)]
mod tests {
    use super::*; extern crate std;
    #[test] fn route() {
        assert_eq!(child_route(0, 3, 0), 3);              // device on tier-0 hub port 3
        assert_eq!(child_route(3, 2, 1), 3 | (2<<4));     // grandchild
    }
    #[test] fn tt() {
        assert!(needs_tt(1, 3));  assert!(needs_tt(2, 3));
        assert!(!needs_tt(3, 3)); assert!(!needs_tt(1, 4));
    }
    #[test] fn portstatus() {
        let s = decode_port_status(0b0000_0100_0000_0011, 0); // connect+enable+high
        assert!(s.connected && s.enabled && s.speed == 3);
    }
    #[test] fn hubdesc() {
        let d = [9,0x29,4, 0x00,0x00, 1, 0,0,0];
        let h = decode_hub_desc(&d).unwrap();
        assert_eq!(h.nbr_ports, 4); assert_eq!(h.pwr_on_2_pwr_good_ms, 2);
    }
}
```

- [ ] **Step 2:** add `pub mod encoding;` to `kernel/src/usb/mod.rs`.
- [ ] **Step 3:** host-test the pure module:
  `wsl ... 'cd /tmp && rm -rf et && mkdir -p et/src && cp /mnt/e/MinimalOS/BasicOperatingSystem/kernel/src/usb/encoding.rs et/src/lib.rs && cd et && printf "[package]\nname=\"et\"\nversion=\"0.0.0\"\nedition=\"2021\"\n" > Cargo.toml && cargo test 2>&1 | tail -6'` → 4 passed.
- [ ] **Step 4:** kernel build clean (`cargo build --release | tail -3` → Finished; dead_code ok).
- [ ] **Step 5:** commit `feat(usb): pure topology/descriptor encoding helpers + tests`.

---

## Task 0b [PAR]: QMP test scripts

Independent shell scripts (verified at the end). Research QEMU nested-USB QMP syntax, write both scripts.

**Files:** Create `tests/usb-hotplug-test.sh`, `tests/usb-hub-test.sh`.

- [ ] **Step 1:** `tests/usb-hotplug-test.sh` — boot `qemu-xhci` (no kbd) + QMP socket; after boot, QMP `device_add usb-kbd,id=k1`; `send-key` "usbhp"; assert echo on serial; `device_del k1`; send keys again; assert NO new echo after del. Pattern from `tests/usb-key-test.sh` (QMP unix socket + python driver). Markers: `TEST_PASS_HOTPLUG`.
- [ ] **Step 2:** `tests/usb-hub-test.sh` — boot with `-device usb-hub,id=h1,bus=xhci.0,port=1 -device usb-kbd,id=k1,bus=xhci.0,port=1.1` (keyboard behind hub at boot); assert `usb hub slot=` + `usb keyboard ready` + a `send-key` "hubkbd" echoes. Marker `TEST_PASS_HUB`. (qemu-xhci needs an id: change the device line to `-device qemu-xhci,id=xhci`.)
- [ ] **Step 3:** commit `test(usb): hot-plug + hub QMP test scripts`. (Scripts are RUN in Task 6 once the feature exists; here just author them. They will fail until then — do not run as a gate now.)

---

## Task 1 [SEQ]: Slot registry + action worklist

The data backbone. No callers yet (dead_code acceptable).

**Files:** Create `kernel/src/usb/registry.rs`; modify `kernel/src/usb/mod.rs`.

- [ ] **Step 1:** Define in `registry.rs`:

```rust
use crate::memory::dma::DmaRegion;
use crate::sync::IrqMutex;
use alloc::collections::VecDeque;

pub const MAX_SLOTS: usize = 256;

pub enum SlotKind {
    Hub(crate::usb::hub::HubState),
    Keyboard(crate::usb::hid::HidState),
    Other,
}

pub struct SlotEntry {
    pub kind: SlotKind,
    pub dev: crate::usb::device::UsbDevice, // EP0 ring + ctx (control-transfer handle)
    pub root_port: u8,
    pub parent_slot: u8,   // 0 = root
    pub parent_port: u8,   // hub port (0 = root)
    pub route: u32,
    pub tier: u8,
    pub speed: u8,
}

const NONE: Option<SlotEntry> = None;
static SLOTS: IrqMutex<[Option<SlotEntry>; MAX_SLOTS]> = IrqMutex::new([NONE; MAX_SLOTS]);

/// Connect/disconnect work, produced by event dispatch, drained by poll().
#[derive(Clone, Copy)]
pub enum UsbAction {
    RootPortChanged(u8),
    HubPortChanged { hub_slot: u8, port: u8 },
}
static WORK: IrqMutex<VecDeque<UsbAction>> = IrqMutex::new(VecDeque::new());

pub fn push_action(a: UsbAction) { WORK.lock().push_back(a); }
pub fn pop_action() -> Option<UsbAction> { WORK.lock().pop_front() }

pub fn insert(slot: u8, e: SlotEntry) { SLOTS.lock()[slot as usize] = Some(e); }
pub fn with_slot<R>(slot: u8, f: impl FnOnce(&mut SlotEntry) -> R) -> Option<R> {
    SLOTS.lock()[slot as usize].as_mut().map(f)
}
/// Find the child slot hanging off (parent_slot, parent_port), if any.
pub fn find_child(parent_slot: u8, port: u8) -> Option<u8> { /* scan SLOTS */ }
```

- [ ] **Step 2:** `teardown(x, slot)`: recursively tear down children first (`for s in 0..MAX_SLOTS where parent_slot==slot`), then `ring::enqueue_cmd(x,[0,0,0,(slot<<24)],10)` (Disable Slot) + `wait_for` completion, clear `DCBAA[slot]=0`, `dma::dealloc` the entry's regions (dev.ep0_ring/input_ctx/dev_ctx + kind-specific rings/buffers), `SLOTS.lock()[slot]=None`. Log `usb teardown slot={}`.

   NB: `teardown` takes `&mut Xhci` (for the command + DCBAA) but must NOT hold the SLOTS lock across `wait_for` (which dispatches events that may lock SLOTS) — collect the regions/children under the lock, release, then issue commands. Document this lock discipline.

- [ ] **Step 3:** `mod.rs`: `pub mod registry;`. Remove the old `DEVICE`/`KBD`/`HID` singletons (keep `CTRL`). Build (expect breakage in callers — fixed in later tasks; or gate with `#[allow(dead_code)]` and keep old code until Task 3 rewires — implementer's judgment to keep it compiling). Commit `feat(usb): slot registry + action worklist`.

---

## Task 2 [SEQ]: Event-dispatch core

Central routing so foreign events are never dropped (enables nested enumeration + hot-plug).

**Files:** Create `kernel/src/usb/xhci/event.rs`; modify `ring.rs`, `control.rs`, `xhci/mod.rs`.

- [ ] **Step 1:** `event.rs`:

```rust
use super::Xhci;
use super::ring;
use crate::usb::registry::{self, UsbAction};

/// Route one event TRB to its handler / the worklist.
pub fn dispatch(x: &mut Xhci, ev: [u32;4]) {
    match ring::trb_type(&ev) {
        32 => { // Transfer Event
            let slot = ((ev[3] >> 24) & 0xFF) as u8;
            let dci  = ((ev[3] >> 16) & 0x1F) as u8;
            // Route by registry kind (keyboard report / hub status change).
            crate::usb::registry::dispatch_transfer(x, slot, dci);
        }
        34 => { // Port Status Change Event: root port = word0 bits 24..31
            let port = ((ev[0] >> 24) & 0xFF) as u8;
            registry::push_action(UsbAction::RootPortChanged(port));
        }
        _ => {} // Command Completion handled by wait_for's predicate; else ignore
    }
}

/// Poll the event ring with a bounded deadline; return the first TRB matching
/// `pred`, dispatching every other TRB so nothing is lost.
pub fn wait_for(x: &mut Xhci, ms: u64, pred: impl Fn(&[u32;4]) -> bool) -> Option<[u32;4]> {
    let start = crate::boot::clock::elapsed_ms();
    while crate::boot::clock::elapsed_ms() - start < ms {
        if let Some(ev) = ring::poll_event(x) {
            if pred(&ev) { return Some(ev); }
            dispatch(x, ev);
        }
        core::hint::spin_loop();
    }
    None
}
```
Add `registry::dispatch_transfer(x, slot, dci)`: `with_slot(slot, ...)` → match kind: Keyboard(st) if dci==st.dci → `hid::on_report(x, st)`; Hub(hs) if dci==hs.dci → `hub::on_status(x, slot, hs)`. (Borrow note: `on_report`/`on_status` need `&mut Xhci` + `&mut state`; `with_slot` already holds SLOTS — pass the state out or run the handler inside the closure WITHOUT re-locking SLOTS. Implementer resolves; document.)

- [ ] **Step 2:** Refactor `ring::wait_cmd` → `event::wait_for(x, 50, |w| trb_type(w)==33)`. Refactor `control_in`/`control_out` wait loops → `wait_for(x, 200/100, |w| trb_type(w)==32)` (so they dispatch foreign events). Keep their completion-code checks on the returned event.
- [ ] **Step 3:** `xhci::poll()` (temporary): `wait_for`-drain loop → `dispatch`. Build clean. Commit `feat(usb): central event dispatch + wait_for routing`.

---

## Task 3 [SEQ]: enumerate(Location) + class dispatch

Replace single-shot addressing with location-parameterized enumeration that registers into the registry.

**Files:** modify `device.rs`, `xhci/mod.rs`, `mod.rs`.

- [ ] **Step 1:** Add `pub struct Location { pub root_port:u8, pub route:u32, pub tier:u8, pub speed:u8, pub parent_slot:u8, pub parent_port:u8, pub tt:bool }`.
- [ ] **Step 2:** `pub fn enumerate(x:&mut Xhci, loc:Location) -> Option<u8>`: Enable Slot (existing code) → slot_id; alloc ep0_ring/dev_ctx/input_ctx; build slot ctx with `set_root_hub_port_number(loc.root_port)`, `set_route_string(loc.route)`, `set_speed(loc.speed)`, and if `loc.tt` `set_parent_hub_slot_id(loc.parent_slot)` + `set_parent_port_number(loc.parent_port)`; `max_packet0 = encoding::max_packet0(loc.speed)`; Address Device; read device descriptor (existing) → class; read config + HID detect (existing `configure` logic) → optional HidKeyboard. Build a `UsbDevice`. Then class dispatch:
  - HID kbd → `hid::configure_endpoint` → `SlotKind::Keyboard(state)`.
  - bDeviceClass 0x09 → `hub::setup(x, slot_id, &mut dev, loc)` → `SlotKind::Hub(state)`.
  - else → `SlotKind::Other`.
  `registry::insert(slot_id, SlotEntry{kind, dev, root_port:loc.root_port, parent_slot:loc.parent_slot, parent_port:loc.parent_port, route:loc.route, tier:loc.tier, speed:loc.speed})`. Log `usb enumerated slot={} kind={} route=0x{:X}`. Return slot_id.
- [ ] **Step 3:** `xhci::init()`: after bring-up, for each connected+reset root port (reuse `scan_ports` reset logic, but per-port) `registry::push_action(RootPortChanged(port))`. Remove the old single-device init flow + DEVICE/KBD/HID stores.
- [ ] **Step 4:** `mod.rs::poll()`: drain events (`wait_for`-less direct loop calling `event::dispatch`), THEN drain the worklist: `while let Some(a)=registry::pop_action() { handle_action(x,a) }` where `handle_action` for `RootPortChanged(p)` resets the root port + builds a root `Location{root_port:p, route:0, tier:0, speed:<port speed>, parent_slot:0, parent_port:0, tt:false}` + `enumerate`; disconnect handling added in Task 5. Build + smoke: root keyboard still works (`usb keyboard ready` + `make run-usb-key-test` green). Commit `feat(usb): location-parameterized enumeration + registry wiring`.

---

## Task 4 [SEQ]: Hub class driver

**Files:** Create `kernel/src/usb/hub.rs`; modify `control.rs` (hub class requests), `mod.rs`.

- [ ] **Step 1:** control.rs hub helpers (operate on `&mut UsbDevice` like control_in/out): `get_hub_descriptor(x,dev,buf)` (Setup 0xA0/6/0x2900/0/len), `get_port_status(x,dev,port)->Option<(u16,u16)>` (Setup 0xA3/0/0/port/4 → parse buf), `set_port_feature(x,dev,port,feat)` (Setup 0x23/3/feat/port/0), `clear_port_feature(x,dev,port,feat)` (Setup 0x23/1/feat/port/0).
- [ ] **Step 2:** `hub.rs`: `pub struct HubState { pub dci:u8, pub nbr_ports:u8, pub int_ring:DmaRegion, pub int_enqueue:usize, pub int_cycle:bool, pub change_buf:DmaRegion }`. `pub fn setup(x,slot,dev,loc)->Option<HubState>`:
  - `get_hub_descriptor` → `encoding::decode_hub_desc` → nbr_ports, tt_think_time, pwr_on_2_pwr_good_ms.
  - One Configure Endpoint (type 12): build Input ctx with A0 (slot: `set_hub(true)`, `set_number_of_ports(nbr_ports)`, `set_tt_think_time(tt)`, keep root_port/route/speed/parent) + A(dci) interrupt-IN status endpoint (dci = 2*(ep&0xF)+1 of the hub's interrupt endpoint from its config desc; allocate int_ring). Wait completion Success.
  - For port 1..=nbr_ports: `set_port_feature(PORT_POWER)`; wait `pwr_on_2_pwr_good_ms`.
  - Alloc `change_buf` (1 page; first ceil((nbr_ports+1)/8) bytes used); queue a Normal TRB on int_ring + ring doorbell(dci).
  - Initial scan: for port 1..=nbr_ports `get_port_status`; if connected `registry::push_action(HubPortChanged{hub_slot:slot, port})`.
  - Log `usb hub slot={} ports={}`. Return HubState.
- [ ] **Step 3:** `pub fn on_status(x, slot, st)`: read `change_buf` bitmap; for each set bit `p` (1..=nbr_ports) `push_action(HubPortChanged{hub_slot:slot, port:p})`; re-queue Normal TRB + doorbell.
- [ ] **Step 4:** `pub fn handle_port(x, hub_slot, port)` (called from worklist): look up hub entry; `get_port_status`; if connected and no existing child: `set_port_feature(PORT_RESET)`, poll `get_port_status` until reset bit clears / C_PORT_RESET, `clear_port_feature(C_PORT_RESET)`+`clear(C_PORT_CONNECTION)`, decode speed, compute child `Location{root_port:hub.root_port, route:encoding::child_route(hub.route,port,hub.tier), tier:hub.tier+1, speed, parent_slot:hub_slot, parent_port:port, tt:encoding::needs_tt(speed,hub.speed)}` (skip if tier+1 >= MAX_TIER), `enumerate(child)`. If disconnected and a child exists: `registry::teardown(x, child)`.
- [ ] **Step 5:** `mod.rs::poll()` worklist: handle `HubPortChanged` → `hub::handle_port`. Build + smoke. Commit `feat(usb): hub class driver (descriptor, ports, status-change)`.

---

## Task 5 [SEQ]: Hot-plug connect/disconnect + teardown wiring

**Files:** modify `mod.rs` (handle_action), `device.rs`/`registry.rs` as needed.

- [ ] **Step 1:** `RootPortChanged(p)` handler: read PORTSC. If connected (CCS) and no slot on root_port p: reset port (scan_ports reset logic) + enumerate root Location. If NOT connected (CCS=0) and a slot exists with root_port==p && parent_slot==0: `registry::teardown(x, slot)`. Clear the PORTSC change bits (CSC) RW1C-safely.
- [ ] **Step 2:** Confirm `dispatch_transfer` routes keyboard reports for ALL keyboards (root + behind hub) and hub status for all hubs. Multiple keyboards: each on_report → master_input_push(0).
- [ ] **Step 3:** Build + run the hot-plug script (`bash tests/usb-hotplug-test.sh`): `TEST_PASS_HOTPLUG` (add at root, type, del, no echo after). Commit `feat(usb): root hot-plug connect/disconnect + teardown`.

---

## Task 6 [SEQ]: QEMU hub topology, gates, changelog

**Files:** modify `Makefile`; create CHANGELOG entry.

- [ ] **Step 1:** Makefile: add `run-usb-hotplug-test` + `run-usb-hub-test` targets (deps: `iso`). Keep existing `run-usb-key-test`. The hub/hotplug tests use their own QEMU lines (with `-qmp` + topology) inside the scripts.
- [ ] **Step 2:** Run all three: `make run-usb-key-test` (root kbd regression), `make run-usb-hub-test` (`TEST_PASS_HUB`), `make run-usb-hotplug-test` (`TEST_PASS_HOTPLUG`). Plus `make run-test` (TEST_PASS — existing markers + `usb keyboard ready` still gate). All green.
- [ ] **Step 3:** CHANGELOG `NNN-26-06-02-usb-hub-hotplug.md` (next number via `ls CHANGELOG | grep -oE '^[0-9]+' | sort -n | tail -1`). Commit `test(usb): hub + hot-plug gates + changelog`.

---

## Final review

Dispatch a reviewer over the branch diff (focus: registry lock discipline — never hold SLOTS across `wait_for`/commands; DMA dealloc on teardown vs use-after-free; recursive teardown ordering; route-string/TT correctness; worklist termination; cycle bits on the new hub interrupt ring). Then `superpowers:finishing-a-development-branch`. Do NOT merge without explicit user approval (CLAUDE.md).

## Self-review notes
- **Spec coverage:** event core (T2), registry+worklist (T1), enumerate+route/TT (T3, T0), hub driver (T4), hot-plug+teardown (T5), tests (T0b,T6). Single-TT only (T4 sets tt_think_time, never multi_tt). PM out (untouched).
- **Parallel boundaries:** T0, T0b are PAR (pure/scripts, no in-tree deps). T1–T6 SEQ (shared coupled files + dependency chain). T0/T0b dispatched concurrently first; then T1→T6 sequential with review.
- **Lock discipline (the top risk):** SLOTS must never be held across `wait_for`/commands (which dispatch events that re-lock SLOTS) → collect-then-release pattern in teardown/dispatch_transfer. Called out in T1/T2.
- **Type consistency:** `Location`, `SlotEntry`/`SlotKind`, `HubState`, `UsbAction`, `child_route`/`needs_tt`/`max_packet0`/`decode_*`, `enumerate`/`teardown`/`dispatch`/`wait_for` consistent across tasks. DCI = 2*(ep&0xF)+1.
- **Honest risk:** the borrow/lock structure around `dispatch_transfer` (handler needs &mut Xhci + &mut state while routing) is the fiddliest part; T2 flags it. Real-HW TT correctness only QEMU-verified.
```