# SMP Fase 0 — per-CPU foundations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make ruos structurally SMP-ready — per-CPU data via GS-base, per-core GDT/TSS/IST (MAX_CPUS=16), ACPI CPU enumeration, and a lock audit + IrqMutex — all exercised on 1 CPU, with NO AP bring-up.

**Architecture:** A `PerCpu` block per core lives in a `static PER_CPU[MAX_CPUS]`; each core's `GsBase` MSR points at its block so `gs:[0]` (a self-pointer) yields `&PerCpu` in O(1). GDT/TSS/IST become per-core arrays indexed by cpu_id; the BSP uses slot 0. ACPI's `platform_info().processor_info` is read to count/identify CPUs (parked, not started). A reusable `IrqMutex<T>` replaces the dangerous `without_interrupts`-as-sync sites found by an audit.

**Tech Stack:** Rust `no_std`, `x86_64` 0.15.4 crate (`registers::model_specific::GsBase`, `structures::gdt`, `structures::tss::TaskStateSegment`), `acpi` crate (`platform_info().processor_info`), `spin`.

---

## Confirmed facts (verified against the tree/crates — use these)

- **GS-base API exists in the x86_64 crate** — `x86_64::registers::model_specific::GsBase::write(addr: VirtAddr)` and `::read() -> VirtAddr` (model_specific.rs:39,348). NO custom rdmsr/wrmsr needed.
- **`gdt::init` zeroes GS** — it calls `GS::set_reg(kernel_data)` (gdt.rs). Therefore `GsBase::write(...)` MUST run AFTER `gdt::init`, or the segment-load clobbers the base. Set gs-base in a step that runs after GDT load.
- **Boot order** (kernel/src/boot/phases/): `arch::init` (Phase 1: `gdt::init` + `idt::init`) → `mem` (parses ACPI: `acpi_init::parse()`) → `interrupts::init` (`lapic::init`). The LAPIC ID needs LAPIC mapped; read it in/after `interrupts`. Put `cpu::init_bsp()` AFTER lapic::init so the APIC ID register (LAPIC + 0x20) is readable, but the gs-base write itself only needs the GDT loaded (already true by Phase 1). Simplest: a single `cpu::init_bsp()` called right after `lapic::init` in `interrupts::init`.
- **`TaskStateSegment::new()` is `const`** (used today as `static mut TSS: TaskStateSegment = TaskStateSegment::new();`), so `[TaskStateSegment::new(); MAX_CPUS]` works as a const array initializer IF `TaskStateSegment: Copy`. CONFIRM `Copy` before coding Task 3: `wsl -d Ubuntu -u root -e bash -c "grep -nE 'derive|struct TaskStateSegment|impl Copy' /root/.cargo/registry/src/*/x86_64-0.15.4/src/structures/tss.rs | head"`. If NOT Copy, use `const NEW: TaskStateSegment = TaskStateSegment::new(); static mut TSS: [TaskStateSegment; MAX_CPUS] = [NEW; MAX_CPUS];` (const-item array repeat works without Copy).
- **`AcpiInfo`** (acpi_init.rs:57) currently has lapic_base/ioapic_base/overrides/ecam/hhdm_offset. You'll ADD a `cpus: Vec<CpuInfo>` field. The `acpi` crate exposes `platform.processor_info: Option<ProcessorInfo>` with `boot_processor: Processor` + `application_processors: Vec<Processor>`; `Processor { processor_uid, local_apic_id, state, is_ap }`. CONFIRM the exact field names: `wsl -d Ubuntu -u root -e bash -c "grep -rnE 'pub struct Processor|pub struct ProcessorInfo|local_apic_id|processor_uid|boot_processor|application_processors|is_ap' /root/.cargo/registry/src/*/acpi-*/src/platform/mod.rs 2>/dev/null | head"`.

Build env: **PowerShell tool** for WSL (git-bash mangles /mnt):
`wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | tail -30'` → clean ends `Limine BIOS stages installed successfully.`. Smoke: `make run-test` → `TEST_PASS` (kill stray qemu first if disk.img busy: `pkill -9 qemu-system-x86; sleep 2`). **Bash tool** for git. CHANGELOG counter: highest is 181 → use 182+.

---

## File structure

- Create `kernel/src/cpu/mod.rs` — `PerCpu`, `PER_CPU`, `MAX_CPUS`, `init_bsp`, `this_cpu`, `cpu_id`.
- Create `kernel/src/sync/mod.rs` — `IrqMutex<T>` (spin::Mutex + IF save/restore).
- Modify `kernel/src/main.rs` — `mod cpu;`, `mod sync;`.
- Modify `kernel/src/gdt.rs` — per-CPU TSS/GDT/IST arrays + `init(cpu_id)`.
- Modify `kernel/src/acpi_init.rs` — `CpuInfo` + `cpus` field, parse processor_info.
- Modify `kernel/src/boot/phases/interrupts.rs` — call `cpu::init_bsp()` after lapic.
- Modify `kernel/src/boot/phases/mem.rs` (or wherever AcpiInfo is consumed) — log CPU count.
- Audit + convert dangerous lock sites (per Task 5's findings).
- `CHANGELOG/NN-26-05-31-*.md` per task; roadmap note.

---

## Task 1: `IrqMutex<T>` reusable IRQ-safe lock

**Files:**
- Create: `kernel/src/sync/mod.rs`
- Modify: `kernel/src/main.rs` (add `mod sync;`)

- [ ] **Step 1: Create `kernel/src/sync/mod.rs`**

```rust
//! Synchronization primitives for ruos.
//!
//! `IrqMutex<T>` = `spin::Mutex<T>` that also disables interrupts for the
//! duration of the lock and restores the prior IF state on drop. Replaces the
//! ad-hoc `without_interrupts(|| some_mutex.lock())` pattern at the sites that
//! actually NEED interrupt masking (shared state touched from both task and
//! ISR context). The spinlock provides cross-core mutual exclusion (SMP-safe);
//! the IF masking prevents an ISR on THIS core from deadlocking on a lock the
//! interrupted task already holds.

use core::ops::{Deref, DerefMut};
use spin::{Mutex, MutexGuard};

pub struct IrqMutex<T> {
    inner: Mutex<T>,
}

pub struct IrqGuard<'a, T> {
    guard: Option<MutexGuard<'a, T>>,
    saved_if: bool,
}

impl<T> IrqMutex<T> {
    pub const fn new(val: T) -> Self {
        Self { inner: Mutex::new(val) }
    }

    /// Lock: save current IF, disable interrupts, then acquire the spinlock.
    /// On guard drop, the spinlock is released and IF is restored.
    pub fn lock(&self) -> IrqGuard<'_, T> {
        let saved_if = x86_64::instructions::interrupts::are_enabled();
        x86_64::instructions::interrupts::disable();
        let guard = self.inner.lock();
        IrqGuard { guard: Some(guard), saved_if }
    }

    /// Non-blocking try-lock. Restores IF immediately if the lock is contended.
    pub fn try_lock(&self) -> Option<IrqGuard<'_, T>> {
        let saved_if = x86_64::instructions::interrupts::are_enabled();
        x86_64::instructions::interrupts::disable();
        match self.inner.try_lock() {
            Some(guard) => Some(IrqGuard { guard: Some(guard), saved_if }),
            None => {
                if saved_if { x86_64::instructions::interrupts::enable(); }
                None
            }
        }
    }
}

// SAFETY: IrqMutex provides mutual exclusion via the inner spin::Mutex.
unsafe impl<T: Send> Send for IrqMutex<T> {}
unsafe impl<T: Send> Sync for IrqMutex<T> {}

impl<'a, T> Deref for IrqGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T { self.guard.as_ref().unwrap() }
}

impl<'a, T> DerefMut for IrqGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T { self.guard.as_mut().unwrap() }
}

impl<'a, T> Drop for IrqGuard<'a, T> {
    fn drop(&mut self) {
        // Release the spinlock FIRST (drop the inner guard), THEN restore IF.
        self.guard = None;
        if self.saved_if {
            x86_64::instructions::interrupts::enable();
        }
    }
}
```

- [ ] **Step 2: Register module.** In `kernel/src/main.rs` add `mod sync;` next to the other top-level `mod` lines.

- [ ] **Step 3: Build.**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | grep -iE "error\[|error:|Limine BIOS stages installed"'`
Expected: `Limine BIOS stages installed successfully.`, no `error`. (Unused-code warnings are fine — it's used in Task 5.)

- [ ] **Step 4: Commit.**
Create `CHANGELOG/182-26-05-31-irqmutex.md` (format like `CHANGELOG/181-26-05-31-host-boundary-fuzz.md`). Summarize: IrqMutex<T> = spin::Mutex + IF save/restore on lock/drop, the reusable IRQ-safe primitive for shared task/ISR state under SMP. Then:
```bash
git add kernel/src/sync/mod.rs kernel/src/main.rs CHANGELOG/182-26-05-31-irqmutex.md
git commit -m "feat(sync): IrqMutex<T> — spin::Mutex + IF save/restore"
```

---

## Task 2: `cpu` module — PerCpu + gs-base

**Files:**
- Create: `kernel/src/cpu/mod.rs`
- Modify: `kernel/src/main.rs` (add `mod cpu;`)

**FIRST — read** `kernel/src/apic/lapic.rs` to find how the LAPIC MMIO base is stored and how to read register 0x20 (Local APIC ID); if there's no ID accessor, you'll add one or read the mapped MMIO directly (the base is passed to `lapic::init`). The APIC ID is `(*(lapic_base + 0x20) >> 24) & 0xFF` for xAPIC.

- [ ] **Step 1: Create `kernel/src/cpu/mod.rs`**

```rust
//! Per-CPU data. Each core's GS base points at its `PerCpu` block, so `gs:[0]`
//! (a self-pointer stored at offset 0) yields `&PerCpu` in O(1) — the standard
//! x86-64 per-CPU pattern. On 1 CPU only slot 0 (the BSP) is live; Fase 1 (AP
//! bring-up) will call `init_ap(n)` for the others.

use x86_64::VirtAddr;
use x86_64::registers::model_specific::GsBase;

pub const MAX_CPUS: usize = 16;

#[repr(C)]
pub struct PerCpu {
    /// MUST be offset 0: a pointer to self, so `mov rax, gs:[0]` loads &PerCpu.
    pub self_ptr: *const PerCpu,
    pub cpu_id: u32,
    pub lapic_id: u32,
    pub kernel_stack_top: u64,
}

impl PerCpu {
    const fn zeroed() -> Self {
        Self { self_ptr: core::ptr::null(), cpu_id: 0, lapic_id: 0, kernel_stack_top: 0 }
    }
}

// SAFETY: PER_CPU is only mutated during single-threaded boot (init_bsp, and
// later init_ap before that AP runs any task). After setup each core only reads
// its own slot via gs-base. Raw-pointer field makes it !Sync by default.
struct PerCpuArray([PerCpu; MAX_CPUS]);
unsafe impl Sync for PerCpuArray {}

static mut PER_CPU: PerCpuArray = PerCpuArray([const { PerCpu::zeroed() }; MAX_CPUS]);

/// Read this core's Local APIC ID (xAPIC: reg 0x20, bits 31:24).
fn read_apic_id() -> u32 {
    // Reuse the lapic module's accessor if present; else read MMIO at base+0x20.
    crate::apic::lapic::apic_id()
}

/// BSP per-CPU init. Call AFTER gdt::init (GS segment-load zeroes the base) and
/// AFTER lapic::init (APIC ID register must be mapped). Sets PER_CPU[0] and the
/// GS base so `this_cpu()` works thereafter.
pub fn init_bsp(kernel_stack_top: u64) {
    let lapic_id = read_apic_id();
    // SAFETY: single-threaded boot; no other accessor to PER_CPU yet.
    unsafe {
        let slot = &mut PER_CPU.0[0];
        slot.cpu_id = 0;
        slot.lapic_id = lapic_id;
        slot.kernel_stack_top = kernel_stack_top;
        slot.self_ptr = slot as *const PerCpu;
        GsBase::write(VirtAddr::new(slot as *const PerCpu as u64));
    }
}

/// &PerCpu for the current core, via gs:[0] (the self-pointer at offset 0).
#[inline]
pub fn this_cpu() -> &'static PerCpu {
    let p: *const PerCpu;
    // SAFETY: init_bsp set GS base to a valid PerCpu whose offset 0 is self_ptr.
    unsafe {
        core::arch::asm!("mov {}, gs:[0]", out(reg) p, options(nostack, preserves_flags, readonly));
        &*p
    }
}

#[inline]
pub fn cpu_id() -> u32 { this_cpu().cpu_id }
```

NOTE: if `crate::apic::lapic::apic_id()` does not exist, add a small `pub fn apic_id() -> u32` to `kernel/src/apic/lapic.rs` that reads the stored base + 0x20 (the base is already kept there for eoi/timer). Do that as part of this task.

- [ ] **Step 2: Register module.** `kernel/src/main.rs`: add `mod cpu;`.

- [ ] **Step 3: Build.** (PowerShell make iso) — expect clean. The `asm!` gs read + `GsBase` must compile.

- [ ] **Step 4: Commit.**
Create `CHANGELOG/183-26-05-31-percpu-gsbase.md`. Summarize: PerCpu block + PER_CPU[MAX_CPUS=16], init_bsp sets GS base, this_cpu()/cpu_id() via gs:[0] self-pointer; lapic::apic_id() accessor. Then:
```bash
git add kernel/src/cpu/mod.rs kernel/src/main.rs kernel/src/apic/lapic.rs CHANGELOG/183-26-05-31-percpu-gsbase.md
git commit -m "feat(cpu): per-CPU data via GS-base (PerCpu, this_cpu, init_bsp)"
```

---

## Task 3: Per-CPU GDT/TSS/IST

**Files:**
- Modify: `kernel/src/gdt.rs`

**FIRST — confirm `TaskStateSegment: Copy`** (see Confirmed-facts grep). Use the const-item array repeat form if not Copy.

- [ ] **Step 1: Convert the statics to per-CPU arrays.** In `kernel/src/gdt.rs`, replace the singular `DOUBLE_FAULT_STACK`, `TSS`, and `GDT` with arrays indexed by cpu_id. Keep `DOUBLE_FAULT_IST_INDEX`/`Selectors`. New shape:

```rust
use crate::cpu::MAX_CPUS;

const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024;

// One double-fault IST stack per core.
static mut DOUBLE_FAULT_STACK: [[u8; DOUBLE_FAULT_STACK_SIZE]; MAX_CPUS] =
    [[0; DOUBLE_FAULT_STACK_SIZE]; MAX_CPUS];

// One TSS per core. Const-item repeat works without Copy.
const NEW_TSS: TaskStateSegment = TaskStateSegment::new();
static mut TSS: [TaskStateSegment; MAX_CPUS] = [NEW_TSS; MAX_CPUS];

// One GDT per core, built lazily in init(cpu_id). Use Once per slot or an
// array of Option built at init. Simplest: per-core Once.
static GDT: [spin::Once<(GlobalDescriptorTable, Selectors)>; MAX_CPUS] =
    [const { spin::Once::new() }; MAX_CPUS];
```

- [ ] **Step 2: `init(cpu_id)`.** Rewrite `pub fn init()` → `pub fn init(cpu_id: usize)`. Body: set `TSS[cpu_id].interrupt_stack_table[DF_IST] = &DOUBLE_FAULT_STACK[cpu_id] end`; build `GDT[cpu_id]` appending the kernel/user segments + `Descriptor::tss_segment(&TSS[cpu_id])`; `gdt.load()`; load segment regs + `load_tss`. Mirror the EXISTING init body exactly, just indexed by `cpu_id` and using `GDT[cpu_id].call_once(...)`. Keep the same `unsafe` justifications (now: "this core's slot, no other accessor").

```rust
pub fn init(cpu_id: usize) {
    use x86_64::instructions::segmentation::{CS, DS, ES, FS, GS, SS, Segment};
    use x86_64::instructions::tables::load_tss;

    // SAFETY: each core touches only its own slot during its own boot.
    unsafe {
        let stack_start = VirtAddr::from_ptr(&raw const DOUBLE_FAULT_STACK[cpu_id]);
        let stack_end   = stack_start + DOUBLE_FAULT_STACK_SIZE as u64;
        TSS[cpu_id].interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = stack_end;
    }

    let (gdt, sels) = GDT[cpu_id].call_once(|| {
        let mut gdt = GlobalDescriptorTable::new();
        let kernel_code = gdt.append(Descriptor::kernel_code_segment());
        let kernel_data = gdt.append(Descriptor::kernel_data_segment());
        let user_code   = gdt.append(Descriptor::user_code_segment());
        let user_data   = gdt.append(Descriptor::user_data_segment());
        let tss = gdt.append(unsafe {
            Descriptor::tss_segment(&*core::ptr::addr_of!(TSS[cpu_id]))
        });
        (gdt, Selectors { kernel_code, kernel_data, user_code, user_data, tss })
    });

    gdt.load();
    unsafe {
        CS::set_reg(sels.kernel_code);
        DS::set_reg(sels.kernel_data);
        ES::set_reg(sels.kernel_data);
        FS::set_reg(sels.kernel_data);
        GS::set_reg(sels.kernel_data);
        SS::set_reg(sels.kernel_data);
        load_tss(sels.tss);
    }
}
```
Update `pub fn selectors()` to take `cpu_id` OR keep a BSP-only `selectors()` returning `GDT[0]`'s — check who calls `selectors()` (`grep -rn 'gdt::selectors' kernel/src/`) and adapt the smallest way (most callers are BSP/early; a `selectors()` returning slot 0 is fine, or `selectors(cpu_id)`).

- [ ] **Step 3: Update the caller.** In `kernel/src/boot/phases/arch.rs`, change `crate::gdt::init();` → `crate::gdt::init(0);` (BSP = slot 0).

- [ ] **Step 4: Build + smoke.** (PowerShell make iso) clean, then `make run-test` → `TEST_PASS`. The #DF IST path is unchanged for the BSP (slot 0), so the double-fault handler still has its clean stack.

- [ ] **Step 5: Commit.**
Create `CHANGELOG/184-26-05-31-percpu-gdt-tss.md`. Summarize: GDT/TSS/double-fault-IST stacks are now per-CPU arrays (MAX_CPUS), `gdt::init(cpu_id)`, BSP uses slot 0; #DF IST per-core. Then:
```bash
git add kernel/src/gdt.rs kernel/src/boot/phases/arch.rs CHANGELOG/184-26-05-31-percpu-gdt-tss.md
git commit -m "feat(gdt): per-CPU GDT/TSS/IST arrays; init(cpu_id), BSP slot 0"
```

---

## Task 4: ACPI CPU enumeration + boot wiring

**Files:**
- Modify: `kernel/src/acpi_init.rs`
- Modify: `kernel/src/boot/phases/interrupts.rs` (call `cpu::init_bsp`)
- Modify: the place that consumes `AcpiInfo` to log the CPU count

**FIRST — confirm** the `acpi` crate `Processor`/`ProcessorInfo` field names (Confirmed-facts grep).

- [ ] **Step 1: Add `CpuInfo` + `cpus` to AcpiInfo.** In `kernel/src/acpi_init.rs`:
```rust
#[derive(Clone, Copy)]
pub struct CpuInfo {
    pub processor_uid: u32,
    pub lapic_id: u32,
    pub is_bsp: bool,
}
```
Add `pub cpus: Vec<CpuInfo>,` to `struct AcpiInfo`.

- [ ] **Step 2: Populate it in `parse()`.** After `let platform = tables.platform_info()...`, extract processor_info:
```rust
    let mut cpus: Vec<CpuInfo> = Vec::new();
    if let Some(pi) = platform.processor_info {
        cpus.push(CpuInfo {
            processor_uid: pi.boot_processor.processor_uid,
            lapic_id: pi.boot_processor.local_apic_id,
            is_bsp: true,
        });
        for ap in pi.application_processors.iter() {
            cpus.push(CpuInfo {
                processor_uid: ap.processor_uid,
                lapic_id: ap.local_apic_id,
                is_bsp: false,
            });
        }
    }
```
Adjust field names to the REAL `acpi` crate (the grep tells you `processor_uid`/`local_apic_id` exact names; `application_processors` may be a `Vec<Processor>`). If `processor_info` is `None`, leave `cpus` empty (BSP-only fallback). Add `cpus` to the `Ok(AcpiInfo { ... })` literal at the end.

- [ ] **Step 3: Log the count + call init_bsp.** In `kernel/src/boot/phases/interrupts.rs`, AFTER `lapic::init(...)`, add the BSP per-CPU init and a count log. You need a kernel stack top for `init_bsp`; pass the current RSP top or a known boot-stack symbol — simplest acceptable value for Fase 0: read current RSP (`x86_64::registers::read_rip` is wrong; use `let rsp: u64; asm!("mov {}, rsp", out(reg) rsp)`), or pass 0 if kernel_stack_top isn't consumed yet by anything (it's a forward-looking field). Use 0 for now with a comment that Fase 1 fills it per-AP:
```rust
    crate::cpu::init_bsp(0); // kernel_stack_top filled per-AP in Fase 1
    crate::binfo!("cpu", "cpu0 apic_id={} gs_base set", crate::cpu::this_cpu().lapic_id);
```
And where AcpiInfo is available (it's parsed in `mem` phase and presumably stored/passed — find it: `grep -rn 'AcpiInfo\|acpi_info\|\.cpus' kernel/src/boot/`), log:
```rust
    crate::binfo!("cpu", "acpi: {} CPU(s) found (1 active, {} parked)", n, n.saturating_sub(1));
```
(where `n = acpi_info.cpus.len().max(1)`). If AcpiInfo isn't easily reachable in interrupts phase, log in the mem phase right after parse() instead — put the count log wherever `acpi_info` is in scope; put `init_bsp` after lapic::init.

- [ ] **Step 4: Build + smoke.** clean + `make run-test` → `TEST_PASS`. Serial should now show `cpu cpu0 apic_id=0 gs_base set` and `cpu acpi: N CPU(s) found`.

- [ ] **Step 5: Verify the smoke markers.**
Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | grep -iE "cpu0 apic_id|CPU\(s\) found"'`
Expected: both lines present (QEMU default is 1 CPU → `1 CPU(s) found`). Optionally re-run with `-smp 4` by editing the run target temporarily to confirm `4 CPU(s) found` (enumeration only; no AP started) — document if you do.

- [ ] **Step 6: Commit.**
Create `CHANGELOG/185-26-05-31-acpi-cpu-enum.md`. Summarize: AcpiInfo.cpus (CpuInfo: uid/lapic_id/is_bsp) from platform_info.processor_info; init_bsp wired after lapic::init; boot logs cpu0 + CPU count (parked, not started). Then:
```bash
git add kernel/src/acpi_init.rs kernel/src/boot/ CHANGELOG/185-26-05-31-acpi-cpu-enum.md
git commit -m "feat(acpi,cpu): enumerate CPUs + BSP per-CPU init at boot"
```

---

## Task 5: Lock audit + convert dangerous sites

**Files:**
- Create: `docs/superpowers/notes/2026-05-31-smp-lock-audit.md` (the audit table)
- Modify: the specific files whose sites are classified "must fix" (determined by the audit)

This is the security/correctness core. Output is (a) a documented classification of ALL shared-state sync sites, (b) conversion of the dangerous ones to `IrqMutex`/proper locks.

- [ ] **Step 1: Enumerate every candidate site.**
Run (PowerShell):
`wsl -d Ubuntu -u root -e bash -c "cd /mnt/e/MinimalOS/BasicOperatingSystem && echo '== without_interrupts =='; grep -rn 'without_interrupts' kernel/src/; echo '== static mut =='; grep -rn 'static mut' kernel/src/; echo '== spin::Mutex statics =='; grep -rn 'static .*Mutex\|: Mutex<' kernel/src/"`
Capture the full output.

- [ ] **Step 2: Classify each into the audit doc.** Create `docs/superpowers/notes/2026-05-31-smp-lock-audit.md` with a table: `site (file:line) | kind | shared with ISR? | current protection | SMP verdict | action`. Verdicts:
  - **SAFE-AS-IS**: `without_interrupts(|| M.lock()…)` where `M` is a `spin::Mutex` — the spinlock gives cross-core exclusion; WI only prevents same-core IRQ deadlock. Keep. (Most pty/pipe/sockets sites are this.)
  - **SAFE-BSP-ONLY**: state only ever touched on the BSP / during single-threaded boot (e.g. gdt one-time init, banner). Keep + note the invariant.
  - **MUST-FIX**: a `static mut` mutated without any lock, OR shared state protected ONLY by `without_interrupts` (IF-based, no spinlock) that is reachable from >1 context. These break under SMP.
  For EACH `static mut`: determine if it's (i) init-once-then-read (boot only — SAFE-BSP-ONLY), (ii) genuinely mutable shared state (MUST-FIX → wrap in `IrqMutex`/`spin::Mutex`). The executor's `EXECUTOR`/`PER_CPU`/GDT/TSS statics are SAFE (documented single-core or per-core invariants) — note them explicitly.

- [ ] **Step 3: Convert the MUST-FIX sites only.** For each MUST-FIX, replace with `crate::sync::IrqMutex` (if ISR-shared) or `spin::Mutex` (if not), updating call sites. Do the SMALLEST correct change per site. Do NOT mass-rewrite SAFE-AS-IS sites (YAGNI — they work). If the audit finds ZERO must-fix sites (plausible — the codebase already uses spin::Mutex widely), that is a valid outcome: document "no unprotected shared state found; IrqMutex available for future use" and the task is the audit doc + that conclusion.

- [ ] **Step 4: Update the executor comment.** In `kernel/src/executor/mod.rs`, expand the `unsafe impl Sync for ExecCell` comment to: "SAFETY: exactly one core calls `run()` (the BSP). The cooperative executor is single-core by design (pivot); Fase 2 revisits this for SMP. PER_CPU/this_cpu give per-core data but the run-queue is not yet SMP-safe." No functional change.

- [ ] **Step 5: Build + ALL smokes.** clean build, then:
`make run-test` → TEST_PASS; `make run-ssh-test` → TEST_PASS_SSH; `make run-pipe-test` → TEST_PASS_PIPE; `make run-fuel-test` → TEST_PASS_FUEL. (Run separately, kill stray qemu between.) The audit conversions must not regress anything.

- [ ] **Step 6: Commit.**
Create `CHANGELOG/186-26-05-31-smp-lock-audit.md`. Summarize: full sync-site audit (N sites: X safe-as-is, Y bsp-only, Z must-fix); converted the Z must-fix to IrqMutex/spin::Mutex; executor documented single-core. Then:
```bash
git add docs/superpowers/notes/2026-05-31-smp-lock-audit.md kernel/src/ CHANGELOG/186-26-05-31-smp-lock-audit.md
git commit -m "audit(smp): classify all sync sites; convert dangerous ones to IrqMutex"
```

---

## Task 6: Roadmap note + final verification

**Files:**
- Modify: `docs/superpowers/roadmap-rust-os.md` (Step 18 SMP — mark Fase 0 done)
- Modify: `README.md` (optional one-line under status if SMP is tracked there)

- [ ] **Step 1: Roadmap note.** In `docs/superpowers/roadmap-rust-os.md`, under the SMP section (Step 18), add a "Fase 0 — per-CPU foundations (DONE)" note: per-CPU data (gs-base), per-core GDT/TSS/IST (MAX_CPUS=16), ACPI CPU enumeration, lock audit + IrqMutex; on 1 CPU, no AP. Link the spec. State Fase 1 (AP bring-up) + Fase 2 (SMP executor) remain.

- [ ] **Step 2: Final full regression.** Run all four (PowerShell, separately, kill qemu between):
`make run-test` (TEST_PASS), `make run-ssh-test` (TEST_PASS_SSH), `make run-pipe-test` (TEST_PASS_PIPE), `make run-fuel-test` (TEST_PASS_FUEL). Paste all four verdicts.

- [ ] **Step 3: Commit.**
Create `CHANGELOG/187-26-05-31-smp-phase0-roadmap.md`. Then:
```bash
git add docs/superpowers/roadmap-rust-os.md README.md CHANGELOG/187-26-05-31-smp-phase0-roadmap.md
git commit -m "docs(smp): mark Fase 0 done in roadmap; all regressions green"
```

---

## Self-review notes (addressed)

- **Spec coverage:** IrqMutex (T1) ✓; per-CPU gs-base (T2) ✓; per-CPU GDT/TSS/IST (T3) ✓; ACPI enum + init_bsp wiring (T4) ✓; lock audit + fixes + executor doc (T5) ✓; roadmap + final regression (T6) ✓. Done-criteria: this_cpu()=cpu0 (T2/T4 smoke), per-CPU GDT BSP slot 0 (T3), ACPI count (T4), audit complete (T5), all tests green (T5/T6), no AP code (none in any task).
- **Ordering risk flagged:** gs-base write AFTER gdt::init (GS segment-load zeroes base) — T2/T4 place init_bsp after lapic::init, which is after arch's gdt::init. Confirmed against boot phase order.
- **API confirmations carried inline:** GsBase (confirmed present), TaskStateSegment Copy (grep in T3), acpi Processor fields (grep in T4) — each at point of use.
- **Acceptable null outcome named:** if the audit finds no MUST-FIX sites, that's a documented valid result (T5 Step 3), not a failure.
- **No AP bring-up anywhere** — explicitly out; init_ap is named as Fase 1 only.
