# 123 — Spec Step 14: virtio-net + DHCP + CSPRNG

**Data:** 2026-05-29

## Cosa
Scritta la spec di design `docs/superpowers/specs/2026-05-29-rust-virtio-net-dhcp-csprng-design.md`
per lo Step 14 (Networking): driver virtio-net via crate `virtio-drivers` 0.13
(discovery con il layer PCI dello Step 13), allocator DMA riusabile (`memory/dma.rs`)
+ `map_io_range`, seconda `Interface` smoltcp Ethernet con client DHCPv4 (loopback
127/8 preservato), CSPRNG `ChaCha20Rng` seedato da RDRAND (`rng.rs` → `random_get`
+ `/dev/random`). Gate `run-test` = lease DHCP `10.0.2.15` da SLIRP.

## Perché
Abilitare traffico di rete reale (verso SSH allo Step 16) riusando le fondamenta
PCI/DMA. Spec unica (scelta utente) per virtio-net + DHCP + CSPRNG.

## File toccati
- docs/superpowers/specs/2026-05-29-rust-virtio-net-dhcp-csprng-design.md
