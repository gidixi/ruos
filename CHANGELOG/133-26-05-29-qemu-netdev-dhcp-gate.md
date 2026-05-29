# 133 — QEMU netdev virtio-net + gate DHCP

**Data:** 2026-05-29

## Cosa
`run`/`run-test`: `-netdev user,id=net0 -device virtio-net-pci,netdev=net0`.
`run-test` asserisce `net: dhcp bound ip=10.0.2.15` (TEST_FAIL_DHCP altrimenti).

## Perché
Gate end-to-end Step 14: virtio-net TX/RX + smoltcp + DHCP lease da SLIRP.
Verificato: dhcp bound ip=10.0.2.15 gw=10.0.2.2.

## File toccati
- Makefile
