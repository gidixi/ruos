# 202 — USB xHCI + tastiera HID

**Data:** 2026-06-01

## Cosa
Stack USB completo: driver xHCI + enumerazione device + tastiera HID boot. Una
tastiera USB ora digita nella shell ruos (console + SSH), stesso seam della PS/2
(`pty::master_input_push(0, byte)`). Verificato end-to-end su QEMU
(`-device qemu-xhci -device usb-kbd`): un keystroke via QMP `send-key` arriva
alla shell.

Architettura (`kernel/src/usb/`):
- **xhci/** — driver controller via crate `xhci` 0.9.2 (no_std, regs+TRB tipizzati,
  Mapper HHDM su `map_io_range`). Bring-up: reset HC, DCBAA, scratchpad,
  command+event ring con cycle-bit, ERST, RUN. `ring.rs`: enqueue command/transfer
  con wrap del Link TRB, poll event ring + update ERDP.
- **device.rs** — port scan/reset, Enable Slot, Input/Device Context (crate
  `::xhci::context`), Address Device, parse config descriptor.
- **control.rs** — control transfer EP0 (Setup/Data/Status TRB): GET_DESCRIPTOR,
  SET_CONFIGURATION, SET_PROTOCOL.
- **hid.rs** — Configure Endpoint (interrupt-IN, DCI=3), accoda Normal TRB,
  parse boot-report 8 byte, edge-detect tasti nuovi, inietta in PTY 0.
- **usage.rs** — HID usage ID → ASCII (shift/ctrl), 4 unit-test host verdi.

Polling (no MSI): bring-up sincrono nel boot phase `usb` (dopo `pci`, non-fatale);
input via `usb_poll_task` (~10ms) che drena l'event ring e dispatcha gli
Transfer Event al keyboard handler. Single device MVP, boot protocol.

Sequenza boot verificata su QEMU:
```
usb xhci up slots=64 ports=8 / noop ok / port 5 connected speed=3
slot 1 enabled / addressed mps0=64 / dev 0627:0001 class=0
HID kbd iface=0 ep=0x81 mps=8 / slot 1 configured / config_ep ok dci=3
set_protocol boot ok / keyboard ready
```

## Perché
HW reale spesso senza PS/2; un OS bootabile da USB deve poter ricevere input da
tastiera USB. Fondamenta dello stack USB (xHCI + enumerazione + transfer) per
storage/altri HID futuri. PS/2 resta intatto — USB è additivo (stesso seam).

## Dettagli crate `xhci` 0.9.2 (per riferimento)
- `set_tr_dequeue_pointer` esige align 64-byte, DCS via `set_dequeue_cycle_state()`
  separato (NON OR-are il bit nell'indirizzo).
- `EndpointType::InterruptIn`; `Input32Byte::new_32byte()` (CSZ=0 su QEMU).
- PORTSC RW1C: usare i setter `set_*`/`clear_*` del crate; azzerare gli altri
  change-bit (set_0) per non perderli nel read-modify-write.

## Limiti noti
- Tastiera singola, boot protocol (report 8-byte fisso, niente parser report
  descriptor). Niente hub, niente hot-replug oltre il log. Frecce non mappate
  (MVP). Mouse: follow-up (manca consumer GUI).
- Polling 10ms: latenza tasti accettabile, non MSI.

## File toccati
- kernel/src/usb/** (nuovo: mod, xhci/{mod,regs,ring}, device, control, hid, usage)
- kernel/src/boot/phases/usb.rs + mod.rs + boot/mod.rs (phase dopo pci)
- kernel/src/executor/mod.rs (usb_poll_task), kernel/src/main.rs (mod usb)
- kernel/Cargo.toml (xhci 0.9.2), Makefile (-device usb-kbd + gate),
  tests/usb-key-test.sh (QMP keystroke), docs spec+plan
