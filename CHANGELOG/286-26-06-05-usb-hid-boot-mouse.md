# 286 — USB HID boot mouse

**Data:** 2026-06-05

## Cosa
Supporto al mouse USB boot-protocol, accanto a tastiera USB e mouse PS/2.

- `usb/mouse.rs` (nuovo): `decode_boot_mouse(&[u8]) -> MouseEvent` (puro) +
  `self_test` boot-checks. Report `[buttons][dx i8][dy i8]`; USB +Y = giù = già
  convenzione `MouseEvent`, niente negazione.
- `mouse::inject(MouseEvent)`: wrapper pub sulla coda mouse condivisa, così il
  mouse USB alimenta lo stesso path che `gfx::fold_mouse` drena (cursore GUI).
- `hid.rs`: `HidKeyboard` → `HidBootEndpoint` con campo `proto` (1=kbd, 2=mouse);
  `HidState` guadagna `report_len`; `configure_endpoint` parametrizza la lunghezza
  del report; nuova `on_report_mouse` (decode + inject + re-queue TRB).
- `device.rs`: `configure` rileva interface HID boot proto 1 o 2; `enumerate`
  dispatcha `SlotKind::Keyboard` o `SlotKind::Mouse`.
- `registry.rs`: `SlotKind::Mouse(HidState)`; `dispatch_transfer` instrada il
  mouse a `on_report_mouse`; teardown libera la sua DMA come la tastiera.

Path tastiera invariato (`report_len = 8`, `on_report` intatto).

## Perché
Feature originale del branch: il mouse USB esterno non veniva gestito (lo stack
dispatchava solo la tastiera HID boot). Spec:
docs/superpowers/specs/2026-06-05-usb-hid-mouse-design.md.

## Verifica
- `usb::mouse::self_test()` (boot-checks): PASS (bottoni, sign-extension, no-flip Y,
  report corto).
- QEMU `-device usb-mouse`: enumera `kind=Mouse` (`hid boot mouse ready`); QMP rel
  motion → `mouse events injected = 67` (path report→inject completo).
- Tastiera USB QEMU: ancora OK (regression).

## File toccati
- kernel/src/usb/mouse.rs (nuovo)
- kernel/src/usb/mod.rs
- kernel/src/usb/hid.rs
- kernel/src/usb/device.rs
- kernel/src/usb/registry.rs
- kernel/src/mouse/mod.rs
- kernel/src/boot/phases/interrupts.rs
- kernel/src/boot/phases/usb.rs
- docs/superpowers/specs/2026-06-05-usb-hid-mouse-design.md
