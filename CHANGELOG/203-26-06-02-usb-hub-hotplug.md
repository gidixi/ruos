# 203 — USB hub + hot-plug

**Data:** 2026-06-02

## Cosa
Lo stack USB ora gestisce **hub** e **hot-plug a runtime**. Una tastiera collegata
all'avvio o a runtime, su porta root o **dietro uno o più hub**, si enumera e
digita nella shell; scollegandola (o l'hub) il device viene smontato pulito.
Verificato su QEMU via QMP.

Rework del modello MVP (single-device, singleton) in un **USB core**:
- **Event-dispatch centrale** (`xhci/event.rs`): `dispatch(ev)` instrada ogni TRB
  dell'event ring — Transfer Event → handler dello slot (via registry), Port
  Status Change (tipo 34) → worklist, Command Completion → al waiter. `wait_for`
  instrada gli eventi non-attesi a `dispatch` invece di scartarli.
- **Registry slot** (`registry.rs`): `SLOTS[256]` di `SlotEntry{kind,dev,route,
  parent_slot,parent_port,...}`, `SlotKind::{Hub,Keyboard,Other}`. Possiede le DMA
  → `teardown` (Disable Slot + dealloc + smontaggio ricorsivo del sottoalbero).
  **Worklist** `UsbAction::{RootPortChanged,HubPortChanged}` drenata da `poll()`.
- **Enumerazione parametrizzata** (`device.rs` `enumerate(Location)`): slot context
  con route string + (se serve) TT single per device full/low-speed dietro hub
  high-speed; dispatch per classe (keyboard/hub/other).
- **Hub class driver** (`hub.rs`): hub descriptor (0x29), un Configure Endpoint
  (hub bit + nports + tt think time + endpoint interrupt status-change), power
  porte, GET_STATUS/SET-CLEAR_FEATURE, scan iniziale + status-change interrupt →
  enumerazione/teardown ricorsivi.

Polling (no MSI). N tastiere supportate (ognuna → `master_input_push(0)`).

## Sequenza verificata (QEMU)
- **Tastiera dietro hub a boot** (`run-usb-hub-test`): `usb hub slot=1 ports=8` →
  child `enumerated slot=2 route=0x1` → `keyboard ready` → token digitato.
- **Hot-plug root** (`run-usb-hotplug-test`): QMP `device_add usb-kbd` → Port
  Status Change → `enumerated` + `keyboard ready` + digita; `device_del` →
  `teardown slot=N`; nessun eco dopo la rimozione.
- Regressioni verdi: `run-usb-key-test` (tastiera root a boot), `run-test`.

## Note di correttezza (fix concorrenza)
- Il wait dei control transfer matcha lo SPECIFICO slot+EP0 (non un type-32
  qualsiasi) → un report tastiera non viene mis-consegnato a un altro transfer.
- `handle_port` non tiene mai il lock SLOTS durante i control transfer/enumerate/
  teardown (copy-out dei campi hub) → niente deadlock col re-lock di `dispatch`.

## Limiti noti
- Single-TT (gli hub multi-TT cadono in fallback sul TT condiviso — basta per HID).
- Niente power-mgmt/suspend/remote-wakeup (ruos non sospende il bus).
- Niente class driver non-HID (storage ecc. solo identificati). Mouse: follow-up.
- Profondità hub bounded a 5 tier (max spec).

## File toccati
- kernel/src/usb/registry.rs, xhci/event.rs (nuovi)
- kernel/src/usb/{device,hub,control,mod}.rs, xhci/{mod,ring}.rs (rework)
- kernel/src/usb/encoding.rs (nuovo: route/TT/decoders + test host)
- Makefile (run-usb-hub-test, run-usb-hotplug-test)
- tests/usb-hub-test.sh, tests/usb-hotplug-test.sh (nuovi)
