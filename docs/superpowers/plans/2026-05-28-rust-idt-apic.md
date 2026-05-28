# Rust IDT/GDT + APIC + Timer + Keyboard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire a real CPU exception infrastructure (GDT/TSS/IDT) plus APIC-based hardware interrupts (LAPIC timer at 100 Hz, PS/2 keyboard on IOAPIC) for the Rust kernel, with the legacy 8259 PIC masked and ACPI parsed via the `acpi` crate.

**Architecture:** Five layered tasks: (1) GDT + TSS with one IST for `#DF`, (2) IDT + exception handlers + static spin-locked serial + `#BP` smoke test, (3) PIC mask + ACPI/MADT parsing, (4) LAPIC/IOAPIC + LAPIC-timer-driven tick counter, (5) PS/2 keyboard via IOAPIC redirect. The test-pass signature progresses milestone-by-milestone, ending with `ruos: ticks=` after Task 4.

**Tech Stack:** Rust nightly (`nightly-2026-05-26`), `x86_64 = "0.15"`, `acpi = "5"`, `spin = "0.9"` (already present), existing `limine` 0.6 + `uart_16550` + `talc`. All build/run via WSL.

---

## Key facts

- All build/run via **WSL Ubuntu** as root, sourcing cargo env:
  ```
  wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && <cmd>'
  ```
  Edit files with Edit/Write on Windows paths. Git in the normal shell. Branch `feature/idt-apic`. Do not push, do not skip hooks.
- The kernel currently boots, claims a 4 MiB heap, runs an alloc smoke test, and halts. Serial prefix is `ruos:`. Makefile asserts `ruos: alloc box=0xCAFEBABE vec=[0, 1, 2, 3, 4]` via `grep -qF`.
- **Spec:** `docs/superpowers/specs/2026-05-28-rust-idt-apic-design.md`.
- **Vector numbers** (used across tasks): `VEC_LAPIC_TIMER = 0x20`, `VEC_KEYBOARD = 0x21`, `VEC_SPURIOUS = 0xFF`. These are constants in `idt.rs`, re-exported to other modules.
- **Integration risk:** `x86_64` (0.15.x), `acpi` (5.x), and Limine 0.6.3 may expose slightly different APIs from the canonical code below. Adapt the minimal lines needed to compile and report what changed; never redesign.
- **The `int3` smoke test runs BEFORE `sti`.** `#BP` is a CPU trap, not a maskable IRQ, so it fires regardless of `IF`. Hardware IRQs need `sti` (only in Task 4).

## File structure

- `kernel/src/gdt.rs` (new) — GDT + TSS + load function.
- `kernel/src/serial.rs` (modify) — promote `Serial` to a global spin-locked writer.
- `kernel/src/kprint.rs` (new) — `kprintln!` macro built on the global writer.
- `kernel/src/idt.rs` (new) — IDT + exception handlers + vector constants.
- `kernel/src/pic.rs` (new) — disable legacy 8259.
- `kernel/src/acpi_init.rs` (new) — RSDP/MADT parsing, returns `AcpiInfo`.
- `kernel/src/apic/mod.rs` (new) — module re-exports.
- `kernel/src/apic/lapic.rs` (new) — LAPIC enable, EOI, timer LVT, calibration.
- `kernel/src/apic/ioapic.rs` (new) — IOAPIC redirection entry programming.
- `kernel/src/timer.rs` (new) — `TICKS` atomic + handler + `init(hz)`.
- `kernel/src/keyboard.rs` (new) — IRQ1 handler reading port `0x60`.
- `kernel/src/main.rs` (modify) — add Limine `RsdpRequest`, wire init sequence + smoke test.
- `kernel/Cargo.toml` (modify) — add `x86_64` and `acpi`.
- `Makefile` (modify, Task 4) — update `HELLO` to `ruos: ticks=`.

---

## Task 1: GDT + TSS

**Files:**
- Modify: `kernel/Cargo.toml`
- Create: `kernel/src/gdt.rs`
- Modify: `kernel/src/main.rs`
- Create: `CHANGELOG/25-26-05-28-gdt-tss.md`

- [ ] **Step 1: Add `x86_64` to dependencies**

In `kernel/Cargo.toml` `[dependencies]`, add (next to `limine`, `uart_16550`, `talc`, `spin`):

```toml
x86_64 = "0.15"
```

(If `0.15` does not resolve, use the latest published `0.x` and record it in the changelog.)

- [ ] **Step 2: Create `kernel/src/gdt.rs`**

```rust
//! Global Descriptor Table + Task State Segment.
//!
//! A flat GDT with kernel + user code/data segments and a TSS descriptor.
//! The TSS reserves IST stack 0 for the Double Fault handler, so a `#DF`
//! triggered while the regular kernel stack is corrupted still has a clean
//! stack to land on (preventing an instant triple-fault).

use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024; // 16 KiB

static mut DOUBLE_FAULT_STACK: [u8; DOUBLE_FAULT_STACK_SIZE] = [0; DOUBLE_FAULT_STACK_SIZE];

static mut TSS: TaskStateSegment = TaskStateSegment::new();

static GDT: spin::Once<(GlobalDescriptorTable, Selectors)> = spin::Once::new();

#[derive(Copy, Clone)]
pub struct Selectors {
    pub kernel_code: SegmentSelector,
    pub kernel_data: SegmentSelector,
    pub user_code:   SegmentSelector,
    pub user_data:   SegmentSelector,
    pub tss:         SegmentSelector,
}

pub fn init() {
    use x86_64::instructions::segmentation::{CS, DS, ES, FS, GS, SS, Segment};
    use x86_64::instructions::tables::load_tss;

    // SAFETY: single-threaded boot, no other accessors to TSS/stack yet.
    unsafe {
        let stack_start = VirtAddr::from_ptr(&raw const DOUBLE_FAULT_STACK);
        let stack_end   = stack_start + DOUBLE_FAULT_STACK_SIZE as u64;
        TSS.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = stack_end;
    }

    let (gdt, sels) = GDT.call_once(|| {
        let mut gdt = GlobalDescriptorTable::new();
        let kernel_code = gdt.append(Descriptor::kernel_code_segment());
        let kernel_data = gdt.append(Descriptor::kernel_data_segment());
        let user_code   = gdt.append(Descriptor::user_code_segment());
        let user_data   = gdt.append(Descriptor::user_data_segment());
        // SAFETY: TSS lives forever in BSS.
        let tss = gdt.append(unsafe { Descriptor::tss_segment(&*core::ptr::addr_of!(TSS)) });
        (gdt, Selectors { kernel_code, kernel_data, user_code, user_data, tss })
    });

    gdt.load();
    // SAFETY: selectors above match the GDT just loaded.
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

pub fn selectors() -> Selectors {
    GDT.get().expect("gdt::init() not called").1
}
```

(If the `x86_64` crate spelling of e.g. `append` differs — older versions used `add_entry` — adjust. The `Descriptor::tss_segment` signature has changed across versions; use whatever the resolved version exposes.)

- [ ] **Step 3: Wire `gdt::init()` in `kmain`**

Edit `kernel/src/main.rs`. Add `mod gdt;` next to the other `mod` lines (after `mod memory;`). In `kmain`, add a call **after `memory::init_heap()` and its log** (so a heap failure still gets logged on the existing path) but **before any IDT use**:

```rust
    gdt::init();
```

- [ ] **Step 4: Build and run smoke test**

```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -6'
```
Expected: serial still shows the existing three `ruos:` lines and ends with `TEST_PASS`. The GDT load is invisible from userland output — its proof of correctness is that the kernel does NOT triple-fault on the segment reloads.

- [ ] **Step 5: Write the changelog entry**

Create `CHANGELOG/25-26-05-28-gdt-tss.md`:

```markdown
# 25 — GDT + TSS (IST per #DF)

**Data:** 2026-05-28

## Cosa
- Aggiunta dep `x86_64` a kernel/Cargo.toml.
- Nuovo modulo `kernel/src/gdt.rs`: GDT (kernel CS/DS, user CS/DS, TSS) + TSS
  con IST stack 0 (16 KiB BSS) per Double Fault. API `init()` + `selectors()`.
- `kmain` chiama `gdt::init()` dopo `init_heap()`, ricarica CS/DS/ES/FS/GS/SS,
  carica TSS.
- Smoke test invariato: il kernel non triple-faulta sui reload dei segmenti.

## Perché
Prerequisito per IDT (Task 2): #DF deve girare su stack IST separato. La GDT
custom è anche prerequisito per Step 8 (ring 3 user mode).

## File toccati
- kernel/Cargo.toml
- kernel/Cargo.lock
- kernel/src/gdt.rs
- kernel/src/main.rs
- CHANGELOG/25-26-05-28-gdt-tss.md
```

- [ ] **Step 6: Commit**

```bash
git add kernel/Cargo.toml kernel/Cargo.lock kernel/src/gdt.rs kernel/src/main.rs \
        CHANGELOG/25-26-05-28-gdt-tss.md
git commit -m "feat(rust): GDT and TSS with IST stack for #DF"
```

---

## Task 2: IDT + exception handlers + static `SERIAL` + `#BP` smoke test

**Files:**
- Modify: `kernel/src/serial.rs`
- Create: `kernel/src/kprint.rs`
- Create: `kernel/src/idt.rs`
- Modify: `kernel/src/main.rs`
- Create: `CHANGELOG/26-26-05-28-idt-exceptions.md`

- [ ] **Step 1: Promote `Serial` to a global spin-locked writer**

Edit `kernel/src/serial.rs`. Keep the existing `Serial` struct and `impl fmt::Write`. Append at the end:

```rust
/// Globally accessible spin-locked serial writer, used by `kprintln!` and by
/// interrupt handlers. Call `SERIAL.lock().init()` exactly once at boot.
pub static SERIAL: spin::Mutex<Serial> = spin::Mutex::new(Serial::new());

// SAFETY: SerialPort accesses only its I/O port, which is exclusive-by-construction.
unsafe impl Send for Serial {}
```

(If `uart_16550::SerialPort` is already `Send` in the resolved version, the `unsafe impl Send` is redundant but harmless; the compiler will error if `Send` cannot be implemented — drop the line in that case.)

If `Serial::new()` is not `const fn`, change the static to `spin::Once<spin::Mutex<Serial>>` and use `call_once` from `SERIAL_INIT`. The canonical talc/limine patterns we already use show `const fn` is the right shape; verify and adapt.

- [ ] **Step 2: Create the `kprintln!` macro**

Create `kernel/src/kprint.rs`:

```rust
//! `kprintln!` macro built on the global serial writer.

#[macro_export]
macro_rules! kprintln {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = writeln!($crate::serial::SERIAL.lock(), $($arg)*);
    }};
}
```

In `kernel/src/main.rs`, add the module:

```rust
mod kprint;
```

(The `#[macro_export]` attribute makes `kprintln!` available crate-wide via `$crate::`. No `use` needed at call sites.)

- [ ] **Step 3: Create the IDT module**

Create `kernel/src/idt.rs`:

```rust
//! Interrupt Descriptor Table and CPU exception handlers.
//!
//! Hardware IRQ vectors (timer, keyboard) are declared as constants here and
//! the handlers themselves live in `timer.rs` / `keyboard.rs`.

use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use crate::{kprintln, gdt};

pub const VEC_LAPIC_TIMER: u8 = 0x20;
pub const VEC_KEYBOARD:    u8 = 0x21;
pub const VEC_SPURIOUS:    u8 = 0xFF;

static IDT: spin::Once<InterruptDescriptorTable> = spin::Once::new();

pub fn init() {
    let idt = IDT.call_once(|| {
        let mut idt = InterruptDescriptorTable::new();

        idt.divide_error.set_handler_fn(de_handler);
        idt.invalid_opcode.set_handler_fn(ud_handler);
        idt.general_protection_fault.set_handler_fn(gp_handler);
        idt.page_fault.set_handler_fn(pf_handler);
        // SAFETY: stack index 0 is configured in gdt.rs.
        unsafe {
            idt.double_fault
                .set_handler_fn(df_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt.breakpoint.set_handler_fn(bp_handler);

        idt
    });
    idt.load();
}

extern "x86-interrupt" fn de_handler(frame: InterruptStackFrame) {
    kprintln!("ruos: #DE at rip=0x{:X}", frame.instruction_pointer.as_u64());
    halt();
}

extern "x86-interrupt" fn ud_handler(frame: InterruptStackFrame) {
    kprintln!("ruos: #UD at rip=0x{:X}", frame.instruction_pointer.as_u64());
    halt();
}

extern "x86-interrupt" fn gp_handler(frame: InterruptStackFrame, code: u64) {
    kprintln!(
        "ruos: #GP rip=0x{:X} err=0x{:X}",
        frame.instruction_pointer.as_u64(), code
    );
    halt();
}

extern "x86-interrupt" fn pf_handler(frame: InterruptStackFrame, code: PageFaultErrorCode) {
    let cr2 = x86_64::registers::control::Cr2::read();
    kprintln!(
        "ruos: #PF rip=0x{:X} cr2=0x{:X} err={:?}",
        frame.instruction_pointer.as_u64(),
        cr2.as_u64(),
        code
    );
    halt();
}

extern "x86-interrupt" fn df_handler(frame: InterruptStackFrame, _code: u64) -> ! {
    kprintln!("ruos: #DF rip=0x{:X}", frame.instruction_pointer.as_u64());
    halt();
}

extern "x86-interrupt" fn bp_handler(frame: InterruptStackFrame) {
    kprintln!("ruos: bp ok rip=0x{:X}", frame.instruction_pointer.as_u64());
    // Resumable — handler returns; CPU continues at rip past the int3 instruction.
}

fn halt() -> ! {
    loop {
        unsafe { core::arch::asm!("cli; hlt") };
    }
}
```

(If `x86_64` 0.15 spells things differently — e.g., `Cr2::read()` returning `Result`, or `set_handler_fn` requiring `unsafe` for some entries — adapt minimally.)

- [ ] **Step 4: Wire in `kmain` and trigger `#BP`**

Edit `kernel/src/main.rs`. Add `mod idt;` next to `mod gdt;`. Replace the existing `kmain` so it:
1. Initializes the static `SERIAL` once (instead of a local `Serial`).
2. Uses `kprintln!` everywhere we previously called `serial.write_str` / `writeln!(serial, ...)`.
3. After `gdt::init()`, calls `idt::init()` and then triggers `int3`.

The full new `kmain` reads:

```rust
#[no_mangle]
unsafe extern "C" fn kmain() -> ! {
    use alloc::boxed::Box;
    use alloc::vec::Vec;

    // Serial first: any failure below must be observable on the wire.
    crate::serial::SERIAL.lock().init();
    kprintln!("ruos: hello serial");

    if !BASE_REVISION.is_supported() {
        kprintln!("ruos: unsupported Limine base revision");
        hcf();
    }

    // Heap init.
    let info = match memory::init_heap() {
        Ok(info) => info,
        Err(e) => {
            kprintln!("ruos: heap fail: {}", e);
            hcf();
        }
    };
    kprintln!("ruos: heap ok base=0x{:X} size={}", info.virt_base, info.size);

    // Smoke test: prove Box and Vec work through the global allocator.
    let b = Box::new(0xCAFEBABEu64);
    let v: Vec<u32> = (0..5).collect();
    kprintln!("ruos: alloc box=0x{:X} vec={:?}", *b, v);

    // Step 5 — interrupt infrastructure.
    gdt::init();
    idt::init();
    kprintln!("ruos: idt up");

    // #BP smoke test: CPU traps are not maskable by IF, so `sti` is not required.
    core::arch::asm!("int3");

    hcf();
}
```

`hcf()` and `panic_handler` stay as-is.

- [ ] **Step 5: Build and run**

```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -10'
```
Expected serial includes:
```
ruos: hello serial
ruos: heap ok base=0x... size=4194304
ruos: alloc box=0xCAFEBABE vec=[0, 1, 2, 3, 4]
ruos: idt up
ruos: bp ok rip=0x...
```
and `TEST_PASS`. The Makefile assertion is still `ruos: alloc box=0xCAFEBABE vec=[0, 1, 2, 3, 4]`; reaching it after the refactor proves `SERIAL` works through the lock.

- [ ] **Step 6: Changelog**

Create `CHANGELOG/26-26-05-28-idt-exceptions.md`:

```markdown
# 26 — IDT + handler eccezioni + SERIAL globale + #BP smoke

**Data:** 2026-05-28

## Cosa
- `kernel/src/serial.rs`: aggiunto `SERIAL: spin::Mutex<Serial>` globale.
- Nuovo `kernel/src/kprint.rs`: macro `kprintln!` su SERIAL globale.
- Nuovo `kernel/src/idt.rs`: IDT + handler `#DE`/`#UD`/`#GP`/`#PF`/`#DF`
  (su IST 0) + `#BP` resumable. Costanti `VEC_LAPIC_TIMER=0x20`,
  `VEC_KEYBOARD=0x21`, `VEC_SPURIOUS=0xFF`.
- `kmain` rifattorizzato per usare `SERIAL` + `kprintln!`; dopo `gdt::init()`
  chiama `idt::init()`, logga `idt up`, triggera `int3` (handler logga
  `bp ok rip=0x...` e ritorna).

## Perché
Step 5 Task 2: kernel sopravvive a errori CPU e ha un canale di logging usabile
dagli handler (mutex statico).

## File toccati
- kernel/src/serial.rs, kernel/src/kprint.rs, kernel/src/idt.rs
- kernel/src/main.rs
- CHANGELOG/26-26-05-28-idt-exceptions.md
```

- [ ] **Step 7: Commit**

```bash
git add kernel/src/serial.rs kernel/src/kprint.rs kernel/src/idt.rs kernel/src/main.rs \
        CHANGELOG/26-26-05-28-idt-exceptions.md
git commit -m "feat(rust): IDT with exception handlers and #BP smoke test"
```

---

## Task 3: PIC mask + ACPI parsing (MADT → LAPIC/IOAPIC base)

**Files:**
- Modify: `kernel/Cargo.toml`
- Create: `kernel/src/pic.rs`
- Create: `kernel/src/acpi_init.rs`
- Modify: `kernel/src/main.rs`
- Create: `CHANGELOG/27-26-05-28-pic-acpi.md`

- [ ] **Step 1: Add `acpi` dep**

In `kernel/Cargo.toml`, add to `[dependencies]`:

```toml
acpi = "5"
```

(If `5` is unresolvable, use the latest published major and record it in the changelog.)

- [ ] **Step 2: Create `kernel/src/pic.rs`**

```rust
//! Disable the legacy 8259 PIC by masking every IRQ on both chips.
//! After this runs, the PIC cannot deliver interrupts; the APIC is the only
//! source allowed to fire vectors into the IDT.

use x86_64::instructions::port::Port;

pub fn disable() {
    let mut master_data: Port<u8> = Port::new(0x21);
    let mut slave_data:  Port<u8> = Port::new(0xA1);
    // SAFETY: masking the legacy PIC is idempotent and never causes spurious IRQs.
    unsafe {
        master_data.write(0xFF);
        slave_data.write(0xFF);
    }
}
```

- [ ] **Step 3: Create `kernel/src/acpi_init.rs`**

```rust
//! ACPI bring-up: take Limine's RSDP, parse it with the `acpi` crate, and
//! extract LAPIC base + IOAPIC base + IRQ source overrides from the MADT.

use acpi::{AcpiHandler, AcpiTables, PhysicalMapping, InterruptModel};
use core::ptr::NonNull;
use alloc::vec::Vec;

#[derive(Clone)]
struct HhdmHandler {
    hhdm_offset: u64,
}

impl AcpiHandler for HhdmHandler {
    unsafe fn map_physical_region<T>(&self, phys: usize, size: usize) -> PhysicalMapping<Self, T> {
        let virt = phys as u64 + self.hhdm_offset;
        // SAFETY: HHDM covers all physical memory; the address is mapped and aligned
        // enough for table headers (acpi crate handles alignment internally).
        PhysicalMapping::new(
            phys,
            NonNull::new(virt as *mut T).expect("acpi: null virt after HHDM"),
            size,
            size,
            self.clone(),
        )
    }
    fn unmap_physical_region<T>(_region: &PhysicalMapping<Self, T>) {
        // HHDM mappings are permanent — nothing to unmap.
    }
}

#[derive(Debug, Copy, Clone)]
pub struct IrqOverride {
    pub source: u8,
    pub global_system_interrupt: u32,
    pub active_low: bool,
    pub level_triggered: bool,
}

pub struct AcpiInfo {
    pub lapic_base:  u64,
    pub ioapic_base: u64,
    pub overrides:   Vec<IrqOverride>,
}

#[derive(Debug)]
pub enum AcpiInitError {
    NoRsdp,
    NoHhdm,
    Parse,
    NoLapic,
    NoIoapic,
}

impl core::fmt::Display for AcpiInitError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AcpiInitError::NoRsdp   => f.write_str("no rsdp"),
            AcpiInitError::NoHhdm   => f.write_str("no hhdm"),
            AcpiInitError::Parse    => f.write_str("parse"),
            AcpiInitError::NoLapic  => f.write_str("no lapic"),
            AcpiInitError::NoIoapic => f.write_str("no ioapic"),
        }
    }
}

pub fn parse() -> Result<AcpiInfo, AcpiInitError> {
    let rsdp_resp = crate::RSDP_REQUEST.response().ok_or(AcpiInitError::NoRsdp)?;
    let hhdm_resp = crate::HHDM_REQUEST.response().ok_or(AcpiInitError::NoHhdm)?;

    let handler = HhdmHandler { hhdm_offset: hhdm_resp.offset };
    let rsdp_addr = rsdp_resp.address as usize;
    // SAFETY: rsdp_addr comes from Limine and points at a valid RSDP structure.
    let tables = unsafe {
        AcpiTables::from_rsdp(handler, rsdp_addr).map_err(|_| AcpiInitError::Parse)?
    };

    let platform = tables.platform_info().map_err(|_| AcpiInitError::Parse)?;
    let apic = match platform.interrupt_model {
        InterruptModel::Apic(a) => a,
        _ => return Err(AcpiInitError::NoLapic),
    };

    let lapic_base  = apic.local_apic_address;
    let ioapic_base = apic.io_apics.first().ok_or(AcpiInitError::NoIoapic)?.address as u64;

    let mut overrides: Vec<IrqOverride> = Vec::new();
    for iso in apic.interrupt_source_overrides.iter() {
        overrides.push(IrqOverride {
            source: iso.isa_source,
            global_system_interrupt: iso.global_system_interrupt,
            active_low: matches!(iso.polarity, acpi::platform::interrupt::Polarity::ActiveLow),
            level_triggered: matches!(iso.trigger_mode, acpi::platform::interrupt::TriggerMode::Level),
        });
    }

    Ok(AcpiInfo { lapic_base, ioapic_base, overrides })
}
```

(The `acpi` crate's 5.x API exposes `platform_info()` -> `PlatformInfo` with `interrupt_model: InterruptModel`. If 5.x uses different names — `Apic` variant fields, `local_apic_address` vs `local_apic_address` — adapt to what the resolved version reports. The intent stays the same.)

- [ ] **Step 4: Add `RsdpRequest` to `kernel/src/main.rs`**

Add `use limine::request::RsdpRequest;` next to the other limine imports. After `static HHDM_REQUEST`, add:

```rust
#[used]
#[link_section = ".requests"]
pub static RSDP_REQUEST: RsdpRequest = RsdpRequest::new();
```

It must be between `_START_MARKER` and `_END_MARKER` (already the case, since we place it next to the other in-bracket requests).

- [ ] **Step 5: Wire `pic::disable()` + `acpi_init::parse()` in `kmain`**

In `kernel/src/main.rs`, add `mod pic;` and `mod acpi_init;` next to the other `mod` lines. After `idt::init()` and the existing `kprintln!("ruos: idt up")` and the `int3` smoke test, add:

```rust
    pic::disable();
    let acpi_info = match acpi_init::parse() {
        Ok(info) => info,
        Err(e) => {
            kprintln!("ruos: acpi fail: {}", e);
            hcf();
        }
    };
    kprintln!(
        "ruos: acpi ok lapic=0x{:X} ioapic=0x{:X} overrides={}",
        acpi_info.lapic_base, acpi_info.ioapic_base, acpi_info.overrides.len()
    );
```

Keep `hcf()` as the final statement.

- [ ] **Step 6: Build and run**

```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -10'
```
Expected serial includes a new line like:
```
ruos: acpi ok lapic=0xFEE00000 ioapic=0xFEC00000 overrides=N
```
plus `TEST_PASS` (Makefile assertion unchanged). If the line is absent or shows `acpi fail: ...`, read `build/serial.log` to identify the failure mode.

- [ ] **Step 7: Changelog**

Create `CHANGELOG/27-26-05-28-pic-acpi.md`:

```markdown
# 27 — PIC disable + ACPI parsing (MADT)

**Data:** 2026-05-28

## Cosa
- Aggiunta dep `acpi` (rust-osdev).
- `kernel/src/pic.rs`: `disable()` maschera tutti i 16 IRQ del 8259
  (master 0x21 = 0xFF, slave 0xA1 = 0xFF).
- `kernel/src/acpi_init.rs`: implementa `AcpiHandler` via HHDM, parsa l'RSDP
  che Limine fornisce (nuova `RsdpRequest`), estrae da MADT
  `lapic_base`, `ioapic_base`, lista `IrqOverride`. API `parse() -> Result<AcpiInfo, AcpiInitError>`.
- `kmain` chiama `pic::disable()` poi `acpi_init::parse()`, logga
  `ruos: acpi ok lapic=0x... ioapic=0x... overrides=N`.

## Perché
Prerequisito per LAPIC/IOAPIC (Task 4): servono gli indirizzi base e gli IRQ
overrides per ridirigere correttamente le linee legacy.

## File toccati
- kernel/Cargo.toml, kernel/Cargo.lock
- kernel/src/pic.rs, kernel/src/acpi_init.rs
- kernel/src/main.rs
- CHANGELOG/27-26-05-28-pic-acpi.md
```

- [ ] **Step 8: Commit**

```bash
git add kernel/Cargo.toml kernel/Cargo.lock kernel/src/pic.rs kernel/src/acpi_init.rs \
        kernel/src/main.rs CHANGELOG/27-26-05-28-pic-acpi.md
git commit -m "feat(rust): disable 8259 PIC and parse ACPI MADT"
```

---

## Task 4: LAPIC + IOAPIC + LAPIC timer @ 100 Hz (TEST_PASS milestone)

**Files:**
- Create: `kernel/src/apic/mod.rs`
- Create: `kernel/src/apic/lapic.rs`
- Create: `kernel/src/apic/ioapic.rs`
- Create: `kernel/src/timer.rs`
- Modify: `kernel/src/main.rs`
- Modify: `Makefile`
- Create: `CHANGELOG/28-26-05-28-apic-timer.md`

- [ ] **Step 1: Create `kernel/src/apic/mod.rs`**

```rust
pub mod lapic;
pub mod ioapic;
```

In `kernel/src/main.rs`, add `mod apic;`.

- [ ] **Step 2: Create `kernel/src/apic/lapic.rs`**

```rust
//! Local APIC: xAPIC MMIO at the per-CPU base. We use it for EOI and the
//! local timer. The base address comes from the MADT.
//!
//! Register offsets (bytes from base, all 32-bit access):
//!   SVR       0xF0  Spurious Interrupt Vector Register
//!   EOI       0xB0  End Of Interrupt
//!   LVT_TIMER 0x320 Local Vector Table for the LAPIC timer
//!   TIMER_INIT  0x380 initial count
//!   TIMER_CUR   0x390 current count
//!   TIMER_DIV   0x3E0 divide configuration

use core::ptr::{read_volatile, write_volatile};
use x86_64::instructions::port::Port;

const REG_EOI:        u32 = 0xB0;
const REG_SVR:        u32 = 0xF0;
const REG_LVT_TIMER:  u32 = 0x320;
const REG_TIMER_INIT: u32 = 0x380;
const REG_TIMER_CUR:  u32 = 0x390;
const REG_TIMER_DIV:  u32 = 0x3E0;

const TIMER_MODE_PERIODIC: u32 = 1 << 17;
const TIMER_MASKED:        u32 = 1 << 16;

static mut LAPIC_VIRT: u64 = 0;

fn reg(off: u32) -> *mut u32 {
    // SAFETY: caller ensured `init` ran.
    unsafe { (LAPIC_VIRT + off as u64) as *mut u32 }
}

pub fn init(phys_base: u64, hhdm_offset: u64, spurious_vector: u8) {
    // SAFETY: single-threaded boot, no other writers to LAPIC_VIRT.
    unsafe {
        LAPIC_VIRT = phys_base + hhdm_offset;
        // Enable LAPIC: set bit 8 in SVR, OR in the spurious vector.
        let cur = read_volatile(reg(REG_SVR));
        write_volatile(reg(REG_SVR), cur | (1 << 8) | spurious_vector as u32);
        // Divide config = 16 (the canonical "no surprises" divisor).
        write_volatile(reg(REG_TIMER_DIV), 0x3);
        // Mask the timer until configured.
        write_volatile(reg(REG_LVT_TIMER), TIMER_MASKED);
    }
}

pub fn eoi() {
    // SAFETY: init ran; EOI is always safe to write.
    unsafe { write_volatile(reg(REG_EOI), 0) };
}

/// Calibrate by running the PIT for `pit_ms` ms (one-shot mode) and counting
/// LAPIC timer ticks elapsed. Returns LAPIC ticks per `pit_ms` ms.
pub fn calibrate(pit_ms: u32) -> u32 {
    const PIT_FREQ_HZ: u32 = 1_193_182;
    let pit_count: u16 = ((PIT_FREQ_HZ as u64 * pit_ms as u64) / 1000) as u16;

    let mut pit_cmd:  Port<u8> = Port::new(0x43);
    let mut pit_ch0:  Port<u8> = Port::new(0x40);

    // SAFETY: ports 0x40/0x43 control the PIT.
    unsafe {
        // PIT channel 0, lobyte/hibyte, mode 0 (interrupt on terminal count).
        pit_cmd.write(0b0011_0000);
        pit_ch0.write((pit_count & 0xFF) as u8);
        pit_ch0.write((pit_count >> 8) as u8);

        // Start LAPIC timer with max count, one-shot (clear periodic bit).
        write_volatile(reg(REG_LVT_TIMER), TIMER_MASKED); // masked one-shot
        write_volatile(reg(REG_TIMER_INIT), 0xFFFF_FFFF);

        // Poll PIT until it reaches zero.
        loop {
            pit_cmd.write(0b1110_0010); // read-back channel 0 status
            let status = pit_ch0.read();
            if status & 0x80 != 0 { break; }
        }

        let remaining = read_volatile(reg(REG_TIMER_CUR));
        // Stop LAPIC counter.
        write_volatile(reg(REG_TIMER_INIT), 0);

        0xFFFF_FFFF - remaining
    }
}

pub fn set_timer_periodic(vector: u8, initial_count: u32) {
    // SAFETY: init ran.
    unsafe {
        write_volatile(reg(REG_LVT_TIMER), TIMER_MODE_PERIODIC | vector as u32);
        write_volatile(reg(REG_TIMER_INIT), initial_count);
    }
}
```

(The PIT polling loop uses the "read-back" command. If your QEMU/VBox firmware has flaky PIT emulation, the loop should still complete within milliseconds. If calibration returns zero, log and bail — handled in `timer.rs`.)

- [ ] **Step 3: Create `kernel/src/apic/ioapic.rs`**

```rust
//! I/O APIC: legacy ISA IRQs are routed here. We translate `irq` through
//! ACPI's IRQ source overrides, then write a 64-bit redirection table entry
//! that fires the requested IDT vector.

use core::ptr::{read_volatile, write_volatile};
use crate::acpi_init::IrqOverride;

const REG_IOREDTBL_BASE: u32 = 0x10;

static mut IOAPIC_VIRT: u64 = 0;

fn ioregsel() -> *mut u32 { unsafe { IOAPIC_VIRT as *mut u32 } }
fn iowin()    -> *mut u32 { unsafe { (IOAPIC_VIRT + 0x10) as *mut u32 } }

fn read(idx: u32) -> u32 {
    // SAFETY: init ran.
    unsafe {
        write_volatile(ioregsel(), idx);
        read_volatile(iowin())
    }
}

fn write(idx: u32, val: u32) {
    // SAFETY: init ran.
    unsafe {
        write_volatile(ioregsel(), idx);
        write_volatile(iowin(), val);
    }
}

pub fn init(phys_base: u64, hhdm_offset: u64) {
    // SAFETY: single-threaded boot.
    unsafe { IOAPIC_VIRT = phys_base + hhdm_offset; }

    // Read max redirection entry from IOAPICVER (index 0x01, bits 16..23).
    let ver = read(0x01);
    let max_redir = ((ver >> 16) & 0xFF) as u32; // count is max+1

    // Mask everything until explicit redirect() calls.
    for i in 0..=max_redir {
        let idx = REG_IOREDTBL_BASE + i * 2;
        write(idx, 1 << 16);     // masked
        write(idx + 1, 0);       // destination APIC id 0
    }
}

fn translate(irq: u8, overrides: &[IrqOverride]) -> (u32, bool, bool) {
    for o in overrides {
        if o.source == irq {
            return (o.global_system_interrupt, o.active_low, o.level_triggered);
        }
    }
    (irq as u32, false, false) // identity: active high + edge
}

pub fn redirect(irq: u8, vector: u8, overrides: &[IrqOverride]) {
    let (gsi, active_low, level) = translate(irq, overrides);
    let idx = REG_IOREDTBL_BASE + gsi * 2;
    let mut low = vector as u32;       // delivery mode 0 (fixed), phys dest, unmasked
    if active_low { low |= 1 << 13; }
    if level      { low |= 1 << 15; }
    write(idx, low);
    write(idx + 1, 0);                  // destination APIC id 0
}
```

- [ ] **Step 4: Create `kernel/src/timer.rs`**

```rust
//! LAPIC timer driver. Calibrates LAPIC frequency via PIT, configures the
//! timer in periodic mode at the requested frequency, and exposes a tick
//! counter consumed by `kmain` for the boot smoke test.

use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::structures::idt::InterruptStackFrame;
use crate::{apic::lapic, idt, kprintln};

pub static TICKS: AtomicU64 = AtomicU64::new(0);

pub extern "x86-interrupt" fn timer_handler(_frame: InterruptStackFrame) {
    TICKS.fetch_add(1, Ordering::Relaxed);
    lapic::eoi();
}

pub fn ticks() -> u64 { TICKS.load(Ordering::Relaxed) }

pub fn init(hz: u32) -> Result<(), &'static str> {
    // Calibrate over 10 ms (100 PIT samples per second).
    let lapic_per_10ms = lapic::calibrate(10);
    if lapic_per_10ms == 0 { return Err("calibration"); }
    let lapic_per_sec = lapic_per_10ms * 100;
    let initial_count = lapic_per_sec / hz;
    if initial_count == 0 { return Err("hz too high"); }

    kprintln!(
        "ruos: lapic calibrated {} ticks/sec, periodic count={}",
        lapic_per_sec, initial_count
    );

    lapic::set_timer_periodic(idt::VEC_LAPIC_TIMER, initial_count);
    Ok(())
}
```

The timer handler is installed in the IDT via Task 4's edit to `idt::init`:

In `kernel/src/idt.rs`, register the timer handler after the existing exception entries (inside `IDT.call_once`):

```rust
        idt[VEC_LAPIC_TIMER as usize].set_handler_fn(crate::timer::timer_handler);
```

(Indexing `idt[...]` requires the `mut` in the `idt` shadowing — already present per the `let mut idt = InterruptDescriptorTable::new();` line. If the resolved `x86_64` version uses a different accessor, e.g. a `interrupts` array, adapt to that path.)

- [ ] **Step 5: Wire APIC + timer in `kmain` and update `sti`**

In `kernel/src/main.rs`, add `mod timer;` next to the other `mod` declarations.

The HHDM offset used by `lapic::init` and `ioapic::init` is the same one `init_heap` consumed; expose it from `acpi_init` so the APIC modules can reuse it.

In `kernel/src/acpi_init.rs`, add to `AcpiInfo`:

```rust
pub hhdm_offset: u64,
```

and set it in `parse`:

```rust
Ok(AcpiInfo { lapic_base, ioapic_base, overrides, hhdm_offset: hhdm_resp.offset })
```

Then in `kmain`, the APIC + timer wiring after the `acpi ok` log becomes:

```rust
    apic::lapic::init(acpi_info.lapic_base, acpi_info.hhdm_offset, idt::VEC_SPURIOUS);
    apic::ioapic::init(acpi_info.ioapic_base, acpi_info.hhdm_offset);
    if let Err(e) = timer::init(100) {
        kprintln!("ruos: timer fail: {}", e);
        hcf();
    }

    x86_64::instructions::interrupts::enable(); // sti

    // Wait for the timer to fire enough times.
    while timer::ticks() < 10 { core::hint::spin_loop(); }

    kprintln!("ruos: ticks={}", timer::ticks());
    hcf();
```

(`ioapic::redirect` for IRQ1 is called by `keyboard::init` in Task 5, not here.)

- [ ] **Step 6: Update the Makefile assertion**

In `Makefile`, change `HELLO` to assert the new success line:

```make
HELLO     := ruos: ticks=
```

The substring assertion (`grep -qF`) matches any `ticks=N` value. The earlier `ruos: alloc box=...` and `ruos: bp ok rip=...` lines remain implicit prerequisites that show up in `build/serial.log`.

- [ ] **Step 7: Build and run (TEST_PASS via timer)**

```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -15'
```
Expected serial:
```
ruos: hello serial
ruos: heap ok base=0x... size=4194304
ruos: alloc box=0xCAFEBABE vec=[0, 1, 2, 3, 4]
ruos: idt up
ruos: bp ok rip=0x...
ruos: acpi ok lapic=0x... ioapic=0x... overrides=N
ruos: lapic calibrated <freq> ticks/sec, periodic count=<count>
ruos: ticks=<N>
```
and `TEST_PASS`. If calibration logs zero or the run hangs at `while ticks() < 10`, the timer is not firing — investigate the IDT entry, the LAPIC SVR enable bit, the EOI write, and the LVT mask bit, in that order.

- [ ] **Step 8: Changelog**

Create `CHANGELOG/28-26-05-28-apic-timer.md`:

```markdown
# 28 — LAPIC + IOAPIC + timer LAPIC @ 100 Hz

**Data:** 2026-05-28

## Cosa
- `kernel/src/apic/lapic.rs`: enable SVR, EOI, configurazione timer LVT,
  calibrazione via PIT one-shot a 10 ms.
- `kernel/src/apic/ioapic.rs`: lettura IOAPICVER, mascher init di tutte le
  redirection entry, `redirect(irq, vector, overrides)` applicando IRQ
  source overrides ACPI.
- `kernel/src/timer.rs`: `TICKS: AtomicU64`, handler timer (incrementa +
  `lapic::eoi`), `init(hz)` calibra e configura timer LAPIC periodico.
- `idt.rs`: registra `timer::timer_handler` su `VEC_LAPIC_TIMER`.
- `kmain`: chiama `apic::lapic::init`, `apic::ioapic::init`, `timer::init(100)`,
  `sti`, busy-wait su `ticks() < 10`, logga `ruos: ticks=N`.
- `Makefile`: `HELLO := ruos: ticks=` — l'assertion d'ora in poi prova
  IDT + APIC + EOI + timer + sti tutti insieme.

## Perché
Task 4 chiude il pezzo "hardware interrupts funzionanti" dello Step 5.

## File toccati
- kernel/src/apic/mod.rs, lapic.rs, ioapic.rs
- kernel/src/timer.rs
- kernel/src/idt.rs (timer entry)
- kernel/src/acpi_init.rs (espone hhdm_offset)
- kernel/src/main.rs
- Makefile
- CHANGELOG/28-26-05-28-apic-timer.md
```

- [ ] **Step 9: Commit**

```bash
git add kernel/src/apic kernel/src/timer.rs kernel/src/idt.rs kernel/src/acpi_init.rs \
        kernel/src/main.rs Makefile CHANGELOG/28-26-05-28-apic-timer.md
git commit -m "feat(rust): LAPIC + IOAPIC + LAPIC timer at 100 Hz"
```

---

## Task 5: PS/2 keyboard handler (manual verification)

**Files:**
- Create: `kernel/src/keyboard.rs`
- Modify: `kernel/src/idt.rs`
- Modify: `kernel/src/main.rs`
- Create: `CHANGELOG/29-26-05-28-keyboard.md`

- [ ] **Step 1: Create `kernel/src/keyboard.rs`**

```rust
//! Minimal PS/2 keyboard: read raw scancodes from port 0x60 and log them.
//! IRQ1 is wired to `VEC_KEYBOARD` via IOAPIC redirection.

use x86_64::instructions::port::Port;
use x86_64::structures::idt::InterruptStackFrame;
use crate::{apic, idt, kprintln};
use crate::acpi_init::IrqOverride;

pub extern "x86-interrupt" fn keyboard_handler(_frame: InterruptStackFrame) {
    let mut data: Port<u8> = Port::new(0x60);
    // SAFETY: 0x60 is the PS/2 controller data port.
    let scancode = unsafe { data.read() };
    kprintln!("ruos: kb scancode=0x{:X}", scancode);
    apic::lapic::eoi();
}

pub fn init(ioapic_base_already_initialized: bool, overrides: &[IrqOverride]) {
    debug_assert!(ioapic_base_already_initialized);
    apic::ioapic::redirect(1, idt::VEC_KEYBOARD, overrides);
}
```

- [ ] **Step 2: Register the keyboard handler in the IDT**

In `kernel/src/idt.rs`, inside `IDT.call_once`, after the timer registration line, add:

```rust
        idt[VEC_KEYBOARD as usize].set_handler_fn(crate::keyboard::keyboard_handler);
```

- [ ] **Step 3: Wire `keyboard::init` in `kmain`**

In `kernel/src/main.rs`, add `mod keyboard;`. In `kmain`, after `timer::init(100)` succeeds and **before** `sti`, add:

```rust
    keyboard::init(true, &acpi_info.overrides);
```

The `true` flag is a doc-only marker showing the caller knows IOAPIC is already initialized.

- [ ] **Step 4: Build and verify (automated + manual)**

Automated:
```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tail -10'
```
Expected: `TEST_PASS` (the `ticks=` assertion is unchanged; no key is pressed in headless run).

Manual (interactive QEMU with display):
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run'
```
Click into the QEMU window, press keys, watch the host terminal for serial. Expected lines like:
```
ruos: kb scancode=0x1E
ruos: kb scancode=0x9E
```
(the second is the release scancode, top bit set).

Or in VirtualBox with `Raw File` serial → tail the log file while pressing keys.

- [ ] **Step 5: Changelog**

Create `CHANGELOG/29-26-05-28-keyboard.md`:

```markdown
# 29 — Tastiera PS/2 via IOAPIC IRQ1

**Data:** 2026-05-28

## Cosa
- `kernel/src/keyboard.rs`: handler `keyboard_handler` legge scancode da
  porta 0x60, logga `ruos: kb scancode=0x...`, manda EOI a LAPIC.
- `idt.rs`: handler registrato su `VEC_KEYBOARD = 0x21`.
- `kmain`: `keyboard::init(true, &acpi_info.overrides)` chiama
  `apic::ioapic::redirect(1, VEC_KEYBOARD, overrides)` prima di `sti`.
- Test automatico headless invariato (TEST_PASS via `ticks=`). Verifica
  manuale: tasto premuto -> linea scancode su seriale.

## Perché
Chiude lo Step 5: kernel reagisce a eventi hardware (timer + tastiera).

## File toccati
- kernel/src/keyboard.rs
- kernel/src/idt.rs
- kernel/src/main.rs
- CHANGELOG/29-26-05-28-keyboard.md
```

- [ ] **Step 6: Commit**

```bash
git add kernel/src/keyboard.rs kernel/src/idt.rs kernel/src/main.rs \
        CHANGELOG/29-26-05-28-keyboard.md
git commit -m "feat(rust): PS/2 keyboard handler via IOAPIC IRQ1"
```

---

## Notes for the implementer

- **All build/run via WSL** with `source $HOME/.cargo/env`.
- **Adapt, don't redesign.** `x86_64` and `acpi` crate APIs drift slightly across versions; expect small import-path adjustments. If the calibration logic refuses to converge in your environment, try widening the PIT interval to 50 ms before changing the algorithm.
- **TEST_PASS milestone is Task 4.** Tasks 1-3 are checkpoints (build green, optional new log lines); Task 5 is a manual-verification capstone.
- **`#BP` smoke test runs BEFORE `sti`.** This is intentional — `int3` is a CPU trap not gated by `IF`. Do not move it after `sti` "for symmetry"; it would still work but the diagnostic separation between IDT install and APIC install would be lost.
