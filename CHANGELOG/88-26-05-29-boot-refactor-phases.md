# 88 — boot phases extraction + kmain shrink

**Data:** 2026-05-29

## Cosa

- Estratte le 6 fasi di init da `kmain` in moduli separati sotto `kernel/src/boot/phases/`:
  - `arch.rs` — GDT + IDT; smoke INT3 gated da `boot-checks`
  - `mem.rs` — heap + ACPI parse + frame allocator + paging mapper; smoke map/write/read/unmap gated da `boot-checks`
  - `interrupts.rs` — PIC disable + LAPIC + IOAPIC + timer 100 Hz + keyboard IRQ1 + STI
  - `devices.rs` — framebuffer console + attach; fb self-test gated da `boot-checks`
  - `fs.rs` — VFS init + modules mount; vfs smoke gated da `boot-checks`
  - `userland.rs` — net init + executor::run (mai ritorna)
- Aggiunto `pub fn run() -> Result<core::convert::Infallible, BootError>` in `boot/mod.rs` che chiama le 6 fasi in ordine.
- `kmain` ridotto da ~240 a ~25 righe: serial init, banner stamp, `boot::run()`, error halt.
- `ACPI` info passata da `mem` a `interrupts` via `static Mutex<Option<AcpiInfo>>` in `phases/mod.rs`.
- Aggiunto `#[derive(Clone)]` a `AcpiInfo` in `acpi_init.rs`.
- Aggiunto `[features] boot-checks = []` in `kernel/Cargo.toml`.
- I log di init ora usano il formato `binfo!`/`bwarn!`/`berr!`.
- Smoke output scomparso dalla build default (feature off).

## Perché

Task 2 del refactor boot: separare le responsabilità di init in moduli autonomi per leggibilità, testabilità futura e maintainability. Prerequisito per T3 (make test-boot + CI gating).

## File toccati

- `kernel/Cargo.toml`
- `kernel/src/main.rs`
- `kernel/src/acpi_init.rs`
- `kernel/src/boot/mod.rs`
- `kernel/src/boot/phases/mod.rs` (nuovo)
- `kernel/src/boot/phases/arch.rs` (nuovo)
- `kernel/src/boot/phases/mem.rs` (nuovo)
- `kernel/src/boot/phases/interrupts.rs` (nuovo)
- `kernel/src/boot/phases/devices.rs` (nuovo)
- `kernel/src/boot/phases/fs.rs` (nuovo)
- `kernel/src/boot/phases/userland.rs` (nuovo)
