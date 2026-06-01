# SMP Fase 1 — AP bring-up → idle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Start the ACPI-enumerated Application Processors via Limine's MpRequest, bring each to a valid per-core state (GDT/TSS/IDT + LAPIC-derived cpu_id), register them online, and park them in an `hlt` idle loop — no scheduler, no IRQs, no work on the APs.

**Architecture:** Limine delivers each AP already in 64-bit long mode to a Rust `extern "C" fn(&MpInfo) -> !` (no hand-rolled trampoline). The BSP assigns dense cpu_ids, fills `PER_CPU[id]`, records a `lapic_id → cpu_id` table, then calls `MpInfo::bootstrap(ap_entry, id)`. Each AP loads its GDT/TSS (slot id) + the shared IDT, marks itself online, and `hlt`s. `cpu_id()` reads the LAPIC ID register (works on every VMM, dodging VirtualBox's gs-base quirk) and indexes the table — no `gs:[0]`.

**Tech Stack:** Rust `no_std`, `limine` 0.6.3 (`MpRequest`/`MpResponse`/`MpInfo::bootstrap`), `x86_64` 0.15.4, `spin`, atomics.

---

## Confirmed facts (verified against the tree/crate — use these)

- **limine 0.6.3 MP API** (`src/mp/x86_64.rs` + `request.rs`):
  - `pub struct MpInfo { pub processor_id: u32, pub lapic_id: u32, /* priv */ }`
  - `impl MpInfo { pub fn bootstrap(&self, address: MpGotoFunction, extra_arg: u64); pub fn extra_argument(&self) -> u64; }`
  - `pub type MpGotoFunction = unsafe extern "C" fn(&MpInfo) -> !;`
  - `pub struct MpRespData { pub flags: u32, pub bsp_lapic_id: u32, /* cpu_count, cpus priv */ }`
  - `impl MpRespData { pub const fn cpus(&self) -> &[&MpInfo]; }`
  - `pub type MpRequest = Request<MpRespData, u64>;` with `impl MpRequest { pub const fn new(flags: u64) -> Self }`. Use `MpRequest::new(0)` (0 = no x2APIC; we keep xAPIC since `lapic::apic_id()` reads reg 0x20).
  - Get the response via the generic `Request` deref → `.get_response()` (same accessor the other requests use; CONFIRM by reading how `MEMMAP_REQUEST`/`RSDP_REQUEST` responses are read in the kernel: `grep -rn 'get_response\|\.response()' kernel/src/`). Match whichever accessor the existing requests use.
- **`cpu_id()` / `this_cpu()` have ZERO callers today** (`grep` came back empty; `gdt::init(cpu_id)` takes cpu_id as a *parameter*, it does not call the fn). So changing their bodies is safe — nothing depends on the old slot-0 behavior yet.
- **`lapic::apic_id() -> u32`** exists (`apic/lapic.rs:97`), reads reg 0x20 via the `reg(off)` helper over `static mut LAPIC_VIRT`. Works on any core once `lapic::init` ran.
- **limine requests** live in `main.rs` as `#[used] #[link_section=".requests"] pub static X: XRequest = XRequest::new();` between `_START_MARKER`/`_END_MARKER`. Add `MP_REQUEST` there.
- **Boot order** (`boot/mod.rs`): arch → mem (parses ACPI) → interrupts (`lapic::init` + `cpu::init_bsp(0)` + ACPI cpu enum) → pci → devices → fs → storage → userland. `smp::bringup()` goes at the END of `interrupts::init` (after init_bsp + enum) OR as its own call right after, with the BSP mapped first.
- **IDT** is `static IDT: spin::Once<InterruptDescriptorTable>` with `init()` doing `IDT.call_once(...).load()`. Add a `load()` that loads the already-built IDT for APs.
- **gdt::init(cpu_id: usize)** already loads per-core GDT/TSS from slot cpu_id (Fase 0). APs call it directly.

Build env: **PowerShell tool** for WSL (`make iso` → `Limine BIOS stages installed successfully.`). Tests via PowerShell. **Bash tool** for git. CHANGELOG counter: highest is 187 → use 188+.

VBox test harness: VBoxManage at `C:\Program Files\Oracle\VirtualBox\VBoxManage.exe`; VM `ruos`, serial→`build/log-vbox.log`, ISO on IDE-1-0. Boot:
`& $vbm startvm ruos --type headless; Start-Sleep 14; & $vbm controlvm ruos poweroff` then read `build/log-vbox.log`. **NOTE:** after committing, `touch kernel/build.rs` before `make iso` so the banner sha refreshes (build.rs rerun-if-changed=../.git/HEAD does NOT fire on branch commits — check the banner sha matches HEAD to confirm you booted the right ISO).

---

## File structure

- Modify `kernel/src/main.rs` — add `MP_REQUEST` (limine), `mod smp;`.
- Modify `kernel/src/idt.rs` — add `pub fn load()`.
- Modify `kernel/src/cpu/mod.rs` — `LAPIC_TO_CPU` table, `cpu_id()`/`this_cpu()` via LAPIC, `set_cpu_mapping`, online tracking; `mod ap;`.
- Create `kernel/src/cpu/ap.rs` — `ap_entry`.
- Create `kernel/src/smp.rs` — `bringup()`.
- Modify `kernel/src/boot/phases/interrupts.rs` — map BSP + call `smp::bringup()`.
- Create `tests/smp-test.sh` + modify `Makefile` — `run-smp-test`.
- `CHANGELOG/NN-26-06-01-*.md` per task; roadmap note.

---

## Task 1: MP_REQUEST + idt::load()

**Files:**
- Modify: `kernel/src/main.rs`
- Modify: `kernel/src/idt.rs`

- [ ] **Step 1: Add the MP request import + static.** In `kernel/src/main.rs`, extend the limine import line and add the static among the other requests (between the start/end markers):
  - Change the import (line ~42) to include `MpRequest`:
    ```rust
    use limine::request::{FramebufferRequest, HhdmRequest, MemmapRequest, MpRequest, RsdpRequest};
    ```
  - Add after `FRAMEBUFFER_REQUEST` (and before `_START_MARKER`... actually requests go between start and end markers — place it next to the others, e.g. after FRAMEBUFFER_REQUEST):
    ```rust
    #[used]
    #[link_section = ".requests"]
    pub static MP_REQUEST: MpRequest = MpRequest::new(0);
    ```
  CONFIRM `MpRequest` is exported at `limine::request::MpRequest` (it is per request.rs). If the path differs, fix to compile.

- [ ] **Step 2: Add `mod smp;` to main.rs** next to the other top-level `mod` lines (smp.rs is created in Task 4, but declaring the module now is fine only if the file exists — so DEFER this line to Task 4). SKIP in Task 1; just note it.

- [ ] **Step 3: Add `idt::load()`.** In `kernel/src/idt.rs`, after `pub fn init()`, add:
```rust
/// Load the already-built IDT on the current core (used by APs). `init()` must
/// have run on the BSP first to build the shared IDT.
pub fn load() {
    IDT.get().expect("idt::init() not called before idt::load()").load();
}
```

- [ ] **Step 4: Build.**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | grep -iE "error\[|error:|Limine BIOS stages installed"'`
Expected: `Limine BIOS stages installed successfully.`, no `error`. (MP_REQUEST unused-warning is fine; consumed in Task 4.)

- [ ] **Step 5: Commit.**
Create `CHANGELOG/188-26-06-01-mp-request-idt-load.md` (format like `CHANGELOG/187-26-05-31-smp-phase0-roadmap.md`). Summarize: added Limine MP_REQUEST (starts APs), idt::load() for AP IDT loading. Then:
```bash
git add kernel/src/main.rs kernel/src/idt.rs CHANGELOG/188-26-06-01-mp-request-idt-load.md
git commit -m "feat(smp): Limine MP_REQUEST + idt::load() for APs"
```

---

## Task 2: cpu_id()/this_cpu() via LAPIC + online tracking

**Files:**
- Modify: `kernel/src/cpu/mod.rs`

**Context:** Today `this_cpu()` returns `&PER_CPU[0]` unconditionally (Fase 0 single-CPU). Now it must resolve the REAL current core via the LAPIC ID (no gs:[0] — VBox-safe). `cpu_id()`/`this_cpu()` have zero existing callers, so this is a pure addition of correct multi-CPU behavior.

- [ ] **Step 1: Add the lapic→cpu table + online counter.** In `kernel/src/cpu/mod.rs`, add near the top (after `MAX_CPUS`):
```rust
use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

/// Sentinel for an unmapped LAPIC ID slot.
const NO_CPU: u8 = 0xFF;

/// lapic_id (xAPIC 8-bit) -> dense cpu_id. Filled by `set_cpu_mapping` at
/// bring-up. Read by `cpu_id()` on every core.
static LAPIC_TO_CPU: [AtomicU8; 256] = {
    const Z: AtomicU8 = AtomicU8::new(NO_CPU);
    [Z; 256]
};

/// Count of APs that have reached `ap_entry` and registered online.
static CPUS_ONLINE: AtomicU32 = AtomicU32::new(0);
```

- [ ] **Step 2: Add mapping + online API.** Add these functions:
```rust
/// Register `lapic_id -> cpu_id` and populate PER_CPU[cpu_id]'s identity.
/// Called by the BSP for itself (id 0) and for each AP before bootstrap.
pub fn set_cpu_mapping(lapic_id: u32, cpu_id: u8) {
    if (lapic_id as usize) < 256 {
        LAPIC_TO_CPU[lapic_id as usize].store(cpu_id, Ordering::SeqCst);
    }
    // SAFETY: each slot is written once during single-threaded bring-up before
    // the corresponding AP starts; the AP only reads its own slot afterwards.
    unsafe {
        let slot = core::ptr::addr_of_mut!(PER_CPU.0[cpu_id as usize]);
        (*slot).cpu_id = cpu_id as u32;
        (*slot).lapic_id = lapic_id;
        (*slot).self_ptr = slot as *const PerCpu;
    }
}

/// An AP (or the BSP) marks itself online.
pub fn mark_online() {
    CPUS_ONLINE.fetch_add(1, Ordering::SeqCst);
}

/// Number of cores that have registered online via `mark_online`.
pub fn cpus_online() -> u32 {
    CPUS_ONLINE.load(Ordering::SeqCst)
}
```

- [ ] **Step 3: Rewrite `cpu_id()` and `this_cpu()` to resolve via LAPIC.** Replace the existing `this_cpu()` and `cpu_id()` bodies:
```rust
/// Dense cpu_id of the current core. Reads the LAPIC ID register (works on
/// every core and every VMM — no `gs:[0]`, dodging VirtualBox's gs-base quirk)
/// and maps it to a dense id. Returns 0 (BSP) if the LAPIC ID isn't mapped yet
/// (e.g. very early boot before bring-up) — safe on a single CPU.
#[inline]
pub fn cpu_id() -> u32 {
    let lapic = crate::apic::lapic::apic_id();
    if (lapic as usize) < 256 {
        let id = LAPIC_TO_CPU[lapic as usize].load(Ordering::SeqCst);
        if id != NO_CPU {
            return id as u32;
        }
    }
    0
}

/// &PerCpu for the current core, resolved via `cpu_id()` (LAPIC-based, no gs).
#[inline]
pub fn this_cpu() -> &'static PerCpu {
    // SAFETY: PER_CPU[cpu_id()] is a valid 'static; cpu_id() is in range
    // (0..MAX_CPUS) by construction of the dense ids set in set_cpu_mapping.
    unsafe { &*core::ptr::addr_of!(PER_CPU.0[cpu_id() as usize]) }
}
```
KEEP `init_bsp`, `gs_usable`, `this_cpu_via_gs` as-is (they're for a future
gs-base-cached fast path; not the active path). If `Ordering` import now
conflicts with an existing `use`, dedupe.

- [ ] **Step 4: Build.**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | grep -iE "error\[|error:|Limine BIOS stages installed"'`
Expected: clean. (set_cpu_mapping/mark_online/cpus_online unused until Task 4/5 — warnings fine.)

- [ ] **Step 5: Commit.**
Create `CHANGELOG/189-26-06-01-cpu-id-lapic.md`. Summarize: cpu_id()/this_cpu() now resolve the current core via the LAPIC ID register + LAPIC_TO_CPU table (VMM-independent, no gs:[0]); set_cpu_mapping/mark_online/cpus_online tracking. Then:
```bash
git add kernel/src/cpu/mod.rs CHANGELOG/189-26-06-01-cpu-id-lapic.md
git commit -m "feat(cpu): LAPIC-based cpu_id()/this_cpu() + online tracking"
```

---

## Task 3: AP entry point

**Files:**
- Create: `kernel/src/cpu/ap.rs`
- Modify: `kernel/src/cpu/mod.rs` (add `pub mod ap;`)

- [ ] **Step 1: Create `kernel/src/cpu/ap.rs`.**
```rust
//! Application Processor entry point. Limine hands each AP here already in
//! 64-bit long mode on a Limine-owned stack; we load this core's GDT/TSS and
//! the shared IDT, register online, then park in `hlt`. Fase 1: no IRQs, no
//! work — the cooperative executor stays single-core on the BSP.

use limine::mp::MpInfo;

/// AP entry. `extra_argument` carries the dense cpu_id we assigned in `bringup`.
///
/// SAFETY: invoked by Limine as the AP's `MpGotoFunction`. The BSP has already
/// called `set_cpu_mapping(lapic_id, cpu_id)` so PER_CPU[cpu_id] and the
/// LAPIC→cpu table entry exist before we run.
pub unsafe extern "C" fn ap_entry(info: &MpInfo) -> ! {
    let cpu_id = info.extra_argument() as usize;
    // Load this core's GDT/TSS (slot cpu_id) and the shared IDT.
    crate::gdt::init(cpu_id);
    crate::idt::load();
    // Register online. cpu_id() will now resolve correctly on this core via the
    // LAPIC ID (set up by the BSP before bootstrap).
    crate::cpu::mark_online();
    // Park. No STI: APs receive no interrupts in Fase 1.
    loop {
        x86_64::instructions::hlt();
    }
}
```
CONFIRM the import path `limine::mp::MpInfo` (the crate is `limine` 0.6.3, module `mp`). If it's `limine::mp::x86_64::MpInfo` or re-exported elsewhere, fix to whatever compiles (check: `grep -rn 'pub use\|pub mod' ~/.cargo/.../limine-0.6.3/src/lib.rs ~/.cargo/.../limine-0.6.3/src/mp/mod.rs`).

- [ ] **Step 2: Register the submodule.** In `kernel/src/cpu/mod.rs`, add `pub mod ap;` near the top.

- [ ] **Step 3: Build.**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | grep -iE "error\[|error:|Limine BIOS stages installed"'`
Expected: clean. (ap_entry unused until Task 4 — warning fine.)

- [ ] **Step 4: Commit.**
Create `CHANGELOG/190-26-06-01-ap-entry.md`. Summarize: cpu/ap.rs ap_entry — loads per-core GDT/TSS + shared IDT, marks online, parks in hlt; no IRQs (Fase 1). Then:
```bash
git add kernel/src/cpu/ap.rs kernel/src/cpu/mod.rs CHANGELOG/190-26-06-01-ap-entry.md
git commit -m "feat(cpu): AP entry point — per-core setup + idle hlt"
```

---

## Task 4: smp::bringup() coordinator + boot wiring

**Files:**
- Create: `kernel/src/smp.rs`
- Modify: `kernel/src/main.rs` (add `mod smp;`)
- Modify: `kernel/src/boot/phases/interrupts.rs` (map BSP + call bringup)

**FIRST — confirm the Limine response accessor.** Read how the kernel reads other limine responses: `wsl -d Ubuntu -u root -e bash -c "grep -rn 'get_response\|\.response()' kernel/src/ | head"` (PowerShell). Use the SAME accessor name for `MP_REQUEST` (it's likely `.get_response()` on the deref'd Request, returning `Option<&MpResponse>`). Also confirm `MpResponse` derefs to `MpRespData` so `.cpus()`/`.bsp_lapic_id` are reachable (read request.rs `Response` deref).

- [ ] **Step 1: Create `kernel/src/smp.rs`.**
```rust
//! SMP bring-up: start the enumerated Application Processors via Limine's
//! MpRequest and wait until they're online. Fase 1 parks each AP in `hlt`
//! (see `cpu::ap::ap_entry`); no scheduler/IRQs yet (Fase 2).

/// Start all non-BSP CPUs and wait (bounded) for them to register online.
pub fn bringup() {
    let resp = match crate::MP_REQUEST.get_response() {
        Some(r) => r,
        None => {
            crate::binfo!("smp", "no MP response — single CPU");
            return;
        }
    };
    let bsp_lapic = resp.bsp_lapic_id;
    // BSP is cpu_id 0; map it so cpu_id() resolves on the BSP too.
    crate::cpu::set_cpu_mapping(bsp_lapic, 0);

    let mut next_id: u8 = 1;
    let mut started: u32 = 0;
    for cpu in resp.cpus() {
        if cpu.lapic_id == bsp_lapic {
            continue; // skip the BSP — it's already running
        }
        if (next_id as usize) >= crate::cpu::MAX_CPUS {
            crate::bwarn!("smp", "more CPUs than MAX_CPUS={}; rest parked", crate::cpu::MAX_CPUS);
            break;
        }
        let id = next_id;
        next_id += 1;
        // Fill PER_CPU[id] + LAPIC→cpu mapping BEFORE the AP starts.
        crate::cpu::set_cpu_mapping(cpu.lapic_id, id);
        // Hand the AP its dense cpu_id via the extra argument.
        cpu.bootstrap(crate::cpu::ap::ap_entry, id as u64);
        started += 1;
    }

    // Bounded wait for all started APs to reach ap_entry.
    let mut spins: u64 = 0;
    while crate::cpu::cpus_online() < started && spins < 200_000_000 {
        core::hint::spin_loop();
        spins += 1;
    }
    let online = crate::cpu::cpus_online();
    if started == 0 {
        crate::binfo!("smp", "no APs to start (1 CPU)");
    } else if online == started {
        crate::binfo!("smp", "{}/{} APs online", online, started);
    } else {
        crate::bwarn!("smp", "{}/{} APs online (timeout)", online, started);
    }
}
```
ADAPT: `crate::MP_REQUEST` path (it's a `pub static` in main.rs → `crate::MP_REQUEST`). `get_response()` → match the real accessor. `resp.bsp_lapic_id` / `resp.cpus()` — if `MpResponse` needs an explicit deref or method, adjust (e.g. `resp.cpus()` may be on the deref target). `cpu.bootstrap(fn, arg)` — `ap_entry` must coerce to `MpGotoFunction = unsafe extern "C" fn(&MpInfo) -> !`; it does (Task 3 signature matches).

- [ ] **Step 2: Declare the module.** In `kernel/src/main.rs` add `mod smp;` next to the other `mod` lines.

- [ ] **Step 3: Wire bringup into boot.** In `kernel/src/boot/phases/interrupts.rs`, at the END of `init()` (after the `cpu0 apic_id`/`CPU(s) found` logs, before `Ok(())`), add:
```rust
    // Start the enumerated APs (Limine MpRequest) and park them idle.
    crate::smp::bringup();
```

- [ ] **Step 4: Build.**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | grep -iE "error\[|error:|Limine BIOS stages installed"'`
Expected: clean.

- [ ] **Step 5: Smoke single-CPU (no regression).**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -3'`
Expected: `TEST_PASS` (QEMU run-test default is 1 CPU → `smp: no APs to start (1 CPU)`; boot unaffected).

- [ ] **Step 6: Smoke multi-CPU in QEMU.**

Run: `wsl -d Ubuntu -u root -e bash -c 'pkill -9 qemu-system-x86; sleep 2; cd /mnt/e/MinimalOS/BasicOperatingSystem && timeout 25 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -boot d -cdrom build/os.iso -serial stdio -display none -no-reboot -m 256 2>&1 | grep -iE "APs online|CPU\(s\) found|#PF|panic|init.sh complete"'`
Expected: `acpi: 4 CPU(s) found`, `smp: 3/3 APs online`, `shell: init.sh complete`, NO `#PF`/`panic`. If APs don't come online: check `ap_entry` is reached (add a temporary `binfo!("smp","ap {} up", cpu_id)` in ap_entry — but mind that binfo from an AP touches the shared serial spinlock, which IS safe under SMP per the Fase 0 audit). If it #PFs on an AP: symbolize the rip (addr2line on the release ELF) before guessing.

- [ ] **Step 7: Commit.**
Create `CHANGELOG/191-26-06-01-smp-bringup.md`. Summarize: smp::bringup() starts non-BSP CPUs via Limine bootstrap (dense cpu_ids, PER_CPU + LAPIC map filled first), bounded wait for online; wired at end of interrupts phase; BSP mapped to id 0. QEMU -smp 4 → 3/3 APs online. Then:
```bash
git add kernel/src/smp.rs kernel/src/main.rs kernel/src/boot/phases/interrupts.rs CHANGELOG/191-26-06-01-smp-bringup.md
git commit -m "feat(smp): bring up APs via Limine bootstrap; wait online; park idle"
```

---

## Task 5: Integration test + VirtualBox verification

**Files:**
- Create: `tests/smp-test.sh`
- Modify: `Makefile` (add `run-smp-test`)

- [ ] **Step 1: Create `tests/smp-test.sh`.**
```bash
#!/usr/bin/env bash
# Integration test: SMP Fase 1 — APs brought up to idle on QEMU -smp 4.
# Asserts the BSP enumerated 4 CPUs AND all 3 APs reached online, with no #PF.
set -u
cd "$(dirname "$0")/.."
ISO=build/os.iso
for pid in $(pgrep -f 'qemu-system-x86_64'); do kill -9 "$pid" 2>/dev/null || true; done
sleep 1
rm -f build/serial-smp.log
timeout 40 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -boot d -cdrom "$ISO" \
  -serial stdio -display none -no-reboot -m 256 \
  > build/serial-smp.log 2>&1 || true
echo "=== smp markers ==="
grep -iE "CPU\(s\) found|APs online|#PF|panic|init.sh complete" build/serial-smp.log || true
if grep -qE "smp +3/3 APs online" build/serial-smp.log \
   && grep -qF "init.sh complete" build/serial-smp.log \
   && ! grep -qE "#PF|KERNEL PANIC" build/serial-smp.log; then
  echo TEST_PASS_SMP
else
  echo TEST_FAIL_SMP; tail -25 build/serial-smp.log; exit 1
fi
```
NOTE: this boots headless with serial→stdio (no disk/net needed for the SMP
check — APs come up during the interrupts phase, well before storage/net). If
the kernel requires the AHCI disk to reach the shell, add the `-drive`/`-device
ahci` lines from `tests/ssh-shell-test.sh`; but the `smp: 3/3 APs online` marker
fires before storage, so the marker assertion alone proves Fase 1 even if the
later boot needs more devices. Adjust if `init.sh complete` doesn't appear
without the disk — then assert only on `3/3 APs online` + no #PF.

- [ ] **Step 2: Add the Makefile target.** After `run-fuel-test` in `Makefile`, add:
```makefile
.PHONY: run-smp-test
run-smp-test: iso
	bash tests/smp-test.sh
```
(Read an existing `run-*-test` target first to match TAB indentation + the `iso` prereq style.)

- [ ] **Step 3: Run the SMP test.**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-smp-test 2>&1 | tail -10'`
Expected: markers show `4 CPU(s) found` + `3/3 APs online`, final `TEST_PASS_SMP`.

- [ ] **Step 4: Full regression.**

Run each (PowerShell, kill qemu between): `make run-test` (TEST_PASS), `make run-ssh-test` (TEST_PASS_SSH), `make run-pipe-test` (TEST_PASS_PIPE), `make run-fuel-test` (TEST_PASS_FUEL). Paste verdicts.

- [ ] **Step 5: VirtualBox verification (the real target).**
Rebuild so the banner sha matches HEAD, then boot on VBox and read the serial:
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && touch kernel/build.rs && make iso 2>&1 | tail -1'
```
Then (PowerShell tool):
```
$vbm="C:\Program Files\Oracle\VirtualBox\VBoxManage.exe"
Remove-Item "E:\MinimalOS\BasicOperatingSystem\build\log-vbox.log" -EA SilentlyContinue
& $vbm startvm "ruos" --type headless; Start-Sleep 16; & $vbm controlvm "ruos" poweroff; Start-Sleep 2
Get-Content "E:\MinimalOS\BasicOperatingSystem\build\log-vbox.log" | Select-String -Pattern 'ruos v0|CPU\(s\) found|APs online|#PF|init.sh complete'
```
Expected (VBox is configured with 4 CPUs): banner sha == HEAD, `acpi: 4 CPU(s)
found`, `smp: 3/3 APs online`, `init.sh complete`, NO `#PF`. If VBox shows fewer
CPUs, the count adapts (e.g. `N/N APs online`). If an AP #PFs ONLY on VBox,
symbolize the rip and check it's not a gs:[0] access (cpu_id() must use LAPIC) —
do NOT guess. Power the VM off when done.

- [ ] **Step 6: Roadmap note + CHANGELOG + commit.**
In `docs/superpowers/roadmap-rust-os.md` under Step 18 SMP, add "Fase 1 — AP
bring-up → idle (✅ DONE)": APs started via Limine MpRequest, per-core
GDT/TSS/IDT, LAPIC-based cpu_id, parked idle; verified QEMU -smp 4 + VBox. Note
Fase 2 (SMP executor/scheduler) remains. Create
`CHANGELOG/192-26-06-01-smp-phase1-test-roadmap.md`. Then:
```bash
git add tests/smp-test.sh Makefile docs/superpowers/roadmap-rust-os.md CHANGELOG/192-26-06-01-smp-phase1-test-roadmap.md
git commit -m "test(smp): run-smp-test (QEMU -smp 4) + VBox verify; roadmap Fase 1 done"
```

---

## Self-review notes (addressed)

- **Spec coverage:** MpRequest + idt::load (T1) ✓; LAPIC cpu_id/this_cpu +
  online tracking (T2) ✓; ap_entry (T3) ✓; bringup + wiring + BSP mapping (T4)
  ✓; integration test + VBox verify + roadmap (T5) ✓. Done-criteria: APs reach
  ap_entry + online (T4 smoke / T5), per-core GDT/TSS/IDT (T3), cpu_id via LAPIC
  no gs (T2), VBox no #PF (T5), all tests green (T5), APs idle no STI (T3).
- **VBox quirk handled:** cpu_id() uses LAPIC reg, never gs:[0] — the exact
  fix-class from Fase 0. T5 Step 5 explicitly checks no #PF on VBox.
- **API risk flagged inline:** the limine response accessor (`get_response` vs
  `.response()`) and `MpInfo`/`MpResponse` deref paths carry a "confirm against
  the crate/kernel" instruction at point of use (T4 FIRST).
- **No AP work / no STI** anywhere — APs only load GDT/TSS/IDT + hlt. Executor
  stays single-core (untouched). Matches the Fase 2 boundary.
- **set_cpu_mapping ordering:** PER_CPU[id] + LAPIC table filled BEFORE
  `bootstrap` (T4 Step 1), so the AP's `cpu_id()`/`gdt::init(cpu_id)` resolve
  correctly the instant it runs. Confirmed.
