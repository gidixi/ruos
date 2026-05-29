# 136 — Task 1: nic module skeleton + PCI probe table

**Data:** 2026-05-29

## Cosa

Task 1 dello spec NIC drivers
(`docs/superpowers/specs/2026-05-29-rust-nic-drivers-design-and-plan.md`):

- Crea `kernel/src/net/nic/mod.rs` con:
  - `NicError` enum (NoDevice, UnsupportedDevice(u16), BarMissing,
    ResetTimeout, LinkDown, Dma) + `Display` impl
  - `NicKind` enum 8-variant (E1000, E1000e, Igb, Igc, Rtl8139,
    Rtl8169, Rtl8125, Tg3) — futuro-proof per il full plan
  - `PROBE_TABLE` const: 27 righe (vendor, device, kind)
    coprendo tutte e 7 le famiglie dello spec
  - `probe_and_init()`: walka `pci::devices()`, logga il primo match
    via `binfo!("nic", "found VVVV:DDDD -> kind (bus=N dev=N fn=N)")`,
    ritorna `None` (Task 1 = solo skeleton, drivers nei task successivi)
- `pub mod nic;` aggiunto a `kernel/src/net/mod.rs`

`Nic` enum corpo-vuoto: variants si aggiungono task-by-task (E1000 in
Task 3, ecc.). Tipo presente cosi quando `NetState` wire-up arriva
(Task 4) non serve refactoring.

## Perché

Skeleton + probe table = foundation per i driver. Logging del match
PCI permette di verificare il riconoscimento del NIC anche senza
driver bindato — debug bare-metal-friendly.

## Test

`make build` → `Finished` (49 warnings preesistenti, niente nuovi
errori).

## File toccati

- kernel/src/net/nic/mod.rs (nuovo, 142 righe)
- kernel/src/net/mod.rs (`pub mod nic;`)
- CHANGELOG/136-26-05-29-nic-skeleton-probe.md (questo)
