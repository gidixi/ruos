# 288 — USB tastiera: eventi tasto anche alla GUI

**Data:** 2026-06-05

## Cosa
La tastiera USB ora alimenta **anche** la GUI (eventi `gfx::push_key`), non solo
la shell testuale (PTY). Mirror esatto del driver PS/2, che già fa entrambi.

- `usb/usage.rs`: `usage_to_scancode(usage) -> Option<u32>` (HID usage page 0x07 →
  PS/2 Set 1 scancode, gli stessi che la GUI consuma; tasti estesi frecce/Home/
  End/Del con la convenzione `0xE0xx` della GUI) + `modifier_scancode(bit)` per i
  modificatori. `scancode_self_test()` (boot-checks).
- `usb/hid.rs::on_report`: oltre al byte terminale nel PTY, emette
  `gfx::push_key(scancode, pressed)` con edge-detect **press e release** per i
  tasti (rep[2..8]) e per i bit modificatori (byte0), così egui traccia lo stato
  Shift/Ctrl/Alt.

## Perché
La GUI legge i tasti dagli eventi `gfx` (kind=0, scancode Set 1), non dal PTY. Il
driver PS/2 spinge sia `gfx::push_key` sia `pty::master_input_push(0)`; la tastiera
USB spingeva solo il PTY → funzionava nella shell ma **non nella GUI**. Aggiunto il
ramo GUI mancante. (Combinato con il pompaggio `usb::poll()` in `fold_mouse`,
changelog 287, la tastiera USB ora funziona dentro la GUI.)

## Verifica
- `usb::usage::scancode_self_test()` (boot-checks): PASS (lettere, cifre, Enter,
  Space, freccia su, F1, modificatori).
- `run-usb-key-test`: PASS (path PTY/shell invariato).

## File toccati
- kernel/src/usb/usage.rs
- kernel/src/usb/hid.rs
- kernel/src/boot/phases/interrupts.rs
