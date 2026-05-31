# 183 — Per-CPU data via GS-base (PerCpu, this_cpu, init_bsp)

**Data:** 2026-05-31

## Cosa

Added `kernel/src/cpu/mod.rs` implementing the standard x86-64 per-CPU pattern:

- `PerCpu` struct (`#[repr(C)]`): `self_ptr` at offset 0 (so `gs:[0]` loads
  `&PerCpu` in O(1)), `cpu_id: u32`, `lapic_id: u32`, `kernel_stack_top: u64`.
- `PER_CPU: PerCpuArray` — static array of `MAX_CPUS = 16` slots; `PerCpuArray`
  is a newtype with `unsafe impl Sync` (access partitioned per-core by construction;
  mutated only during single-threaded boot).
- `init_bsp(kernel_stack_top)` — fills `PER_CPU[0]` (reading LAPIC ID via
  `lapic::apic_id()`), writes the self-pointer at offset 0, then calls
  `GsBase::write()` so `this_cpu()` works from that point on. Must be called after
  `gdt::init` and `lapic::init`.
- `this_cpu() -> &'static PerCpu` — inline asm `mov {}, gs:[0]` with
  `options(nostack, preserves_flags)`; dereferences the self-pointer.
- `cpu_id() -> u32` — thin wrapper over `this_cpu().cpu_id`.

`lapic::apic_id() -> u32` accessor added to `kernel/src/apic/lapic.rs`:
reads the Local APIC ID register at offset `0x20` via the existing `reg()`
helper + `read_volatile`, then shifts right 24 bits to extract bits 31:24.

`mod cpu;` added to `kernel/src/main.rs` (after `mod sync;`).

All new items produce only expected dead_code warnings (wired into boot in Task 4).
Build is clean: `Limine BIOS stages installed successfully.`

## Perché

Task 2 of the SMP phase-0 foundations feature. The GS-base self-pointer pattern
gives O(1) per-CPU data access without a CPUID call on every hot path — the same
approach used by Linux (`gs:current_task`) and Windows (`gs:KPCR`). On 1 CPU only
slot 0 (the BSP) is live; AP bring-up is a later phase.

## File toccati
- kernel/src/cpu/mod.rs (new)
- kernel/src/apic/lapic.rs
- kernel/src/main.rs
- CHANGELOG/183-26-05-31-percpu-gsbase.md (this file)
