# 283 — Diagnostica USB `usb-probe` per hardware reale

**Data:** 2026-06-05

## Cosa
Feature cargo `usb-probe` (off di default). Quando attiva, la boot phase USB:
- drena `usb::poll()` in modo sincrono per 3s (l'enumerazione gira QUI, mentre la
  console framebuffer è ancora testo a livello INFO, prima che userland alzi la
  soglia a WARN e la GUI prenda il framebuffer);
- stampa un summary di una schermata: porte root (connesse + speed) e slot
  enumerati (`slot/porta/speed/kind`);
- HALT, così una macchina senza seriale può essere fotografata.

Aggiunti: `usb::probe_ports()` e `usb::registry::probe_dump()` (entrambi
`#[cfg(feature = "usb-probe")]`).

`reset_root_port` (device.rs): sotto `usb-probe` logga il dump PORTSC grezzo
PRE e POST reset (ccs/ped/pr/prc/pls/pp/speed), una sola coppia per porta
(bitmask atomica) per non floodare lo schermo nel retry-loop.

## Perché
Tastiera USB esterna non vista su PC reale (boot completo, cursore lampeggia, ma
nessuna interazione, niente seriale). I log di enumerazione su HW reale scrollano
via / vengono coperti dalla GUI. La diagnostica congela a schermo lo stato USB
reale così si vede DOVE diverge: enumerazione fallita (slot assente / `Other`) vs
tastiera enumerata a Full/Low speed (bug encoding interval endpoint, TODO in
`usb/hid.rs`).

## File toccati
- kernel/Cargo.toml
- kernel/src/boot/phases/usb.rs
- kernel/src/usb/mod.rs
- kernel/src/usb/registry.rs
- kernel/src/usb/device.rs
