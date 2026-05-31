# 184 — Per-CPU GDT/TSS/IST stacks; gdt::init(cpu_id), BSP slot 0

**Data:** 2026-05-31

## Cosa

Converted the three GDT-related statics from single values to per-CPU arrays
sized `MAX_CPUS` (imported from `crate::cpu`):

- `DOUBLE_FAULT_STACK: [[u8; 16 KiB]; MAX_CPUS]` — one IST stack per core in
  BSS (~256 KiB total at MAX_CPUS=16).
- `TSS: [TaskStateSegment; MAX_CPUS]` — one TSS per core. `TaskStateSegment`
  derives `Copy`; used a `const NEW_TSS` repeat-expression for the array
  initialiser.
- `GDT: [spin::Once<(GlobalDescriptorTable, Selectors)>; MAX_CPUS]` — one
  lazily-built GDT+Selectors per core; used a `const ONCE` repeat-expression.

`pub fn init()` → `pub fn init(cpu_id: usize)`: sets the #DF IST pointer in
`TSS[cpu_id]`, builds and loads `GDT[cpu_id]` via `spin::Once::call_once`,
then sets all segment registers. All static-mut accesses use
`core::ptr::addr_of!` / `addr_of_mut!` to avoid the `static_mut_refs` lint.

`pub fn selectors()` now returns slot 0 (`GDT[0].get()...`); all existing
callers are BSP/early, so the signature is unchanged.

`kernel/src/boot/phases/arch.rs`: `crate::gdt::init()` → `crate::gdt::init(0)`
(BSP = slot 0).

Build: clean (`Limine BIOS stages installed successfully.`).
Smoke: `make run-test` → `TEST_PASS`.

## Perché

Task 3 of the SMP phase-0 foundations feature. Each CPU needs its own GDT
(GDTR register is per-core), its own TSS (TR register is per-core), and its
own #DF IST stack (otherwise two cores faulting simultaneously would clobber
each other's emergency stack). On 1 CPU only slot 0 (the BSP) is live; AP
bring-up will call `init(n)` in a later phase.

## File toccati
- kernel/src/gdt.rs
- kernel/src/boot/phases/arch.rs
- CHANGELOG/184-26-05-31-percpu-gdt-tss.md (this file)
