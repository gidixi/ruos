# 253 — Driver mouse PS/2 (IRQ12)

**Data:** 2026-06-04

## Cosa
Aggiunto driver mouse PS/2: `decode_packet` puro (pacchetti 3-byte → MouseEvent,
sign-extension + Y-flip + bottoni), coda eventi IRQ-safe (`pop_event`,
`event_count`), ISR IRQ12 con assemblatore pacchetti + sync guard, sequenza init
controller (enable aux, IRQ12 nel config byte, defaults+reporting) e wiring
IOAPIC. Nuovo vettore IDT `0x22`. Self-test boot-checks su decode+coda
(`mouse decode self-test ok`); init ACK 0xFA/0xFA verificato in QEMU.

## Perché
Prerequisito #1 del desktop egui (input mouse). Indipendente dal gate Wasmtime;
piano docs/superpowers/plans/2026-06-04-mouse-ps2-driver.md.

## File toccati
- kernel/src/mouse/mod.rs
- kernel/src/main.rs
- kernel/src/idt.rs
- kernel/src/boot/phases/interrupts.rs
