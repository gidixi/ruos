# 280 — Fix: encoding interval endpoint HID speed-aware

**Data:** 2026-06-05

## Cosa
Nuova `xhci_interval(speed, b_interval)` in kernel/src/usb/hid.rs: converte il
`bInterval` del descrittore endpoint nel campo `Interval` del contesto endpoint
xHCI (periodo = `2^Interval` microframe da 125 µs), in base alla speed del device.
`configure_endpoint` ora usa il valore convertito invece di passare `bInterval`
grezzo.

- High-speed/SuperSpeed: `Interval = bInterval - 1` (bInterval è già un esponente
  di microframe).
- Full/Low-speed: `bInterval` è in **frame** (1 ms = 8 microframe), quindi
  `Interval = floor(log2(bInterval * 8))`, clamp [3, 15].

## Perché
Una tastiera USB reale Low-speed enumerava (`keyboard ready`) ma non scriveva mai.
Il suo `bInterval=24` (24 ms in frame) veniva passato grezzo al campo Interval →
periodo `2^24` microframe (minuti tra un poll e l'altro) → l'endpoint interrupt-IN
non veniva mai servito, nessun report. La conversione corretta dà Interval=7
(16 ms). Il TODO era già annotato nel codice. Verificato che la tastiera High-speed
in QEMU continua a funzionare (`run-usb-key-test` PASS).

## File toccati
- kernel/src/usb/hid.rs
