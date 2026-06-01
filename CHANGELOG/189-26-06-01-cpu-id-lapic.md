# 189 — LAPIC-based cpu_id()/this_cpu() + online tracking

**Data:** 2026-06-01

## Cosa
- `cpu_id()` and `this_cpu()` now resolve the current core via the LAPIC ID
  register (reads MMIO reg 0x20 via `crate::apic::lapic::apic_id()`) mapped
  through a `LAPIC_TO_CPU: [AtomicU8; 256]` table. No `gs:[0]` used — VMM-
  independent, VirtualBox-safe.
- Added `set_cpu_mapping(lapic_id, cpu_id)`: populates `LAPIC_TO_CPU` and
  fills `PER_CPU[cpu_id]` identity fields (`cpu_id`, `lapic_id`, `self_ptr`)
  using `addr_of_mut!` (no &static-mut). Called by BSP for itself and for
  each AP before the AP starts.
- Added `mark_online()` / `cpus_online()`: atomic counter tracking how many
  cores have registered themselves online.
- Merged all atomic imports into one `use core::sync::atomic::{AtomicBool,
  AtomicU8, AtomicU32, Ordering}` line (no duplicate `Ordering` import);
  updated `GS_USABLE` declaration and all call sites accordingly.
- `init_bsp`, `this_cpu_via_gs`, `gs_usable` left intact (future gs-cached
  fast path, not the active path).

## Perché
SMP Phase 1 Task 2: every AP needs to resolve its own per-CPU block without
relying on the GS segment base (VirtualBox ignores `wrmsr GS_BASE` for the
hidden segment base, making `gs:[0]` unreliable). The LAPIC ID register is
the authoritative, VMM-independent source of core identity.

## File toccati
- kernel/src/cpu/mod.rs
