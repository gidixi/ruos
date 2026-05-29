# 135 — NIC drivers spec (real-HW networking)

**Data:** 2026-05-29

## Cosa

Commit spec design + plan per driver NIC reali:
`docs/superpowers/specs/2026-05-29-rust-nic-drivers-design-and-plan.md`.

7 famiglie: Intel e1000/e1000e, RTL8139, RTL8169/8168/8111, RTL8125,
Intel igb (I210/I211/I350), Intel igc (I225/I226), Broadcom tg3
(BCM57xx). Tier 1 QEMU-testable: e1000, e1000e, rtl8139, igb. Tier
3 real-HW only: rtl8169 family, RTL8125, igc, tg3.

Architettura `net/nic/`: shared descriptor-ring engine + un modulo
per famiglia, ognuno espone smoltcp::phy::Device. Probe table
PCI (vendor/device) → driver.

## Perché

Step 14 (virtio-net) copre solo VM. Bare-metal + VBox (NIC Intel
default) richiedono driver hardware reali. Sblocca dev workflow
senza riconfig adapter VBox.

## Scope MVP scelto: Task 1-4 (e1000 only)

Solo e1000:
- Sblocca VBox default Intel PRO/1000 + Intel onboard LAN bare-metal
- ~3-5 giorni vs ~2-4 settimane full plan
- Resto (e1000e, rtl8139, igb, rtl8169, igc, tg3) rimandato post-MVP
  riusando lo `ring.rs` provato

Plan d'implementazione integrato nello stesso doc (Task 1-12 con
checkbox). Branch lavoro: `feature/nic-e1000`.

## File toccati

- docs/superpowers/specs/2026-05-29-rust-nic-drivers-design-and-plan.md
- CHANGELOG/135-26-05-29-nic-drivers-spec.md (questo)
