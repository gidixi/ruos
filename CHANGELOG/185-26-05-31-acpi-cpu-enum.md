# 185 — ACPI CPU enumeration + BSP per-CPU init at boot

**Data:** 2026-05-31

## Cosa
- Added `CpuInfo { processor_uid: u32, lapic_id: u32, is_bsp: bool }` to
  `acpi_init.rs` and a `pub cpus: Vec<CpuInfo>` field on `AcpiInfo`.
- `parse()` now populates `cpus` from `platform.processor_info` (acpi crate
  `ProcessorInfo<Global>`): BSP from `boot_processor`, APs from
  `application_processors.iter()`. Empty if no MADT processor entries.
- `boot/phases/interrupts.rs`: after `lapic::init()`, calls
  `crate::cpu::init_bsp(0)` (sets GS base so `this_cpu()` works), then logs:
  - `cpu0 apic_id=<N> gs_base set`
  - `acpi: <N> CPU(s) found (<1> active, <N-1> parked)`
- APs are enumerated but NOT started (no INIT-SIPI-SIPI).

## Perché
Task 4 of SMP phase-0: wire BSP per-CPU GS base into the boot sequence and
expose CPU count from ACPI so later AP bring-up has the lapic_id list ready.

## File toccati
- kernel/src/acpi_init.rs
- kernel/src/boot/phases/interrupts.rs
