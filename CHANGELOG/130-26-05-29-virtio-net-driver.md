# 130 — net/virtio.rs: driver virtio-net + smoltcp Device

**Data:** 2026-05-29

## Cosa
`net/virtio.rs`: discovery via PCI (vendor 0x1AF4/class 0x02), `VirtIONet`
(virtio-drivers, PciTransport su MmioCam ECAM), adapter `smoltcp::phy::Device`
(copy+recycle RX, tx via new_tx_buffer/send). `pci::ecam_virt_base()`.
`pci/ecam.rs`: aggiunto `first_base()` su `EcamAccess`.

## Perché
Task 6 di Step 14 (networking): driver virtio-net necessario per smoltcp su
hardware reale/QEMU. Il modulo è compilato ma non ancora wired in `NetState`
(Task 7).

## File toccati
- kernel/src/net/virtio.rs
- kernel/src/net/mod.rs
- kernel/src/pci/mod.rs
- kernel/src/pci/ecam.rs
