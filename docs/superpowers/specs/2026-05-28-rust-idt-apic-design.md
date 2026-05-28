# Rust IDT/GDT + APIC + Timer + Keyboard — Design Spec

**Date:** 2026-05-28
**Milestone:** Step 5 of the Rust OS roadmap (`docs/superpowers/roadmap-rust-os.md`).
**Status:** Approved design, ready for implementation planning.

## Context

The kernel currently boots via Limine, initializes a serial COM1 driver, claims
a 4 MiB heap via `talc`, runs a `Box`/`Vec` smoke test, and halts. It has no
CPU exception handlers, no TSS, no working interrupt infrastructure, and no
hardware-event reaction. A page fault, double fault, or invalid opcode would
triple-fault and reboot the machine.

This milestone installs a real interrupt foundation:

- A proper **GDT** (kernel CS/DS + user CS/DS + TSS selector) and a **TSS**
  with one IST stack reserved for double faults.
- A populated **IDT** with handlers for the most common CPU exceptions, plus a
  resumable breakpoint handler used as a self-test.
- **APIC**-based hardware interrupts (xAPIC MMIO), with the legacy 8259 PIC
  fully masked. ACPI tables are parsed via the `acpi` crate, starting from the
  RSDP that Limine provides through a new request.
- A **LAPIC timer** ticking at 100 Hz, with a Rust handler that increments an
  atomic counter and signals EOI.
- A **PS/2 keyboard** handler reading raw scancodes from port `0x60`, logging
  them, and signalling EOI.

Once this is in place, later milestones can rely on interrupts being usable
(Step 6 page-fault handling, Step 7 preemptive scheduling driven by the timer
IRQ, Step 8 syscall return paths).

## Goals

- The kernel survives the listed CPU exceptions instead of triple-faulting.
- A `#BP` breakpoint round-trips: handler logs, returns, kernel continues.
- The 8259 PIC is fully masked; the APIC drives all hardware interrupts.
- The LAPIC timer fires at 100 Hz and increments a Rust-level tick counter.
- A boot-time smoke test prints a `ruos: ticks=N` line with `N >= 10` after a
  short delay, asserted by `make run-test`.
- PS/2 keyboard scancodes are logged on serial when keys are pressed
  (interactive verification in QEMU/VirtualBox).

## Non-goals (YAGNI)

- No SMP / multi-CPU initialization.
- No HPET. PIT is used briefly to calibrate the LAPIC timer, then masked.
- No MSI/MSI-X.
- No ACPI power management, no AML interpreter.
- No keymap decoding — only raw scancodes.
- No general-purpose `interrupts` abstraction over LAPIC/IOAPIC beyond what
  this milestone needs.
- No syscalls (Step 8), no scheduler (Step 7), no paging changes (Step 6).

## Architecture

Boot sequence in `kmain` after the existing serial/heap setup:

```
serial -> hello
heap   -> "heap ok"
alloc smoke test
gdt::init()                  # load GDT + TSS, set TSS selector
idt::init()                  # load IDT (exception handlers)
pic::disable()               # mask all 16 legacy IRQs on 8259 master+slave
let acpi_info = acpi::parse(rsdp)
                             # RSDP comes from Limine RsdpRequest
                             # crate `acpi` parses MADT -> LAPIC base, IOAPIC base,
                             # IRQ source overrides
lapic::init(acpi_info.lapic_base)
                             # enable SVR, configure timer LVT, calibrate via PIT
ioapic::init(acpi_info.ioapic_base, &acpi_info.irq_overrides)
                             # mask all entries, will unmask per-IRQ as needed
timer::init(100 /* Hz */)    # LAPIC timer periodic mode at requested frequency
keyboard::init()             # unmask IRQ1 -> redirected to chosen vector
x86_64::instructions::interrupts::enable()   # sti
unsafe { core::arch::asm!("int3"); }         # triggers #BP -> logs + returns
busy_wait_ticks(>= 10)
log "ruos: ticks={N}"
hcf()
```

The "busy wait" is a poll loop on the timer's atomic tick counter.

## Components

All new modules live under `kernel/src/`. They are small, single-purpose, and
expose narrow APIs.

### `kernel/src/gdt.rs`

- `pub fn init()` — installs the GDT and loads CS/DS/TSS.
- Internal: `GlobalDescriptorTable` with kernel code/data, user code/data, and a
  TSS descriptor (from the `x86_64` crate).
- Internal: a `TaskStateSegment` with one IST entry pointing at a static
  16 KiB stack in BSS (used by the `#DF` handler).

### `kernel/src/idt.rs`

- `pub fn init()` — installs the IDT (`InterruptDescriptorTable::load`).
- Handlers (`extern "x86-interrupt"` functions) for:
  - `#DE` (vector 0) — log and halt.
  - `#UD` (vector 6) — log and halt.
  - `#GP` (vector 13) — log error code and halt.
  - `#PF` (vector 14) — log CR2 + error code and halt.
  - `#DF` (vector 8) — IST stack; log and halt.
  - `#BP` (vector 3) — log `ruos: bp ok rip=0x...` and **return** (resumable).
- IRQ vectors for timer and keyboard are also wired here but defined in
  `timer.rs` and `keyboard.rs` to keep responsibilities local. `idt.rs`
  re-exports the chosen vector numbers as constants
  (e.g. `pub const VEC_LAPIC_TIMER: u8 = 0x20; pub const VEC_KEYBOARD: u8 = 0x21;`).

### `kernel/src/pic.rs`

- `pub fn disable()` — writes `0xFF` to both master (`0x21`) and slave (`0xA1`)
  data ports, masking all legacy IRQs. The 8259 still exists but cannot
  generate interrupts to the CPU.

### `kernel/src/acpi_init.rs`

- New Limine request `RsdpRequest` lives in `main.rs` (alongside the other
  requests, inside the markers).
- `pub fn parse() -> Result<AcpiInfo, AcpiInitError>`:
  - Reads the RSDP physical address from the Limine response.
  - Hands it to the `acpi` crate via a small `AcpiHandler` impl that maps
    physical addresses by adding the HHDM offset (we already have it via
    `HHDM_REQUEST`).
  - Parses the MADT and returns `AcpiInfo { lapic_base, ioapic_base, irq_overrides }`.
- `AcpiInitError { NoRsdp, NoHhdm, AcpiError, NoLapic, NoIoapic }` with `Display`.

### `kernel/src/apic/mod.rs` + `lapic.rs` + `ioapic.rs`

- `lapic.rs`:
  - `pub fn init(base: u64)` — enables the LAPIC (set bit 8 in Spurious
    Interrupt Vector Register at offset `0xF0`).
  - `pub fn eoi()` — writes `0` to EOI register at offset `0xB0`.
  - `pub fn set_timer_periodic(vector: u8, count: u32)` — configures LVT
    timer (offset `0x320`) with periodic mode and the given vector, sets
    Initial Count (offset `0x380`).
  - `pub fn calibrate(pit_ms: u32) -> u32` — uses the PIT for a `pit_ms`
    one-shot interval, samples Current Count (`0x390`), returns LAPIC ticks
    per `pit_ms` ms. Used by `timer::init`.
  - Internal: MMIO read/write helpers using `core::ptr::{read_volatile,
    write_volatile}` on `(base + offset) as *mut u32`.
- `ioapic.rs`:
  - `pub fn init(base: u64, overrides: &[IrqOverride])` — masks all 24
    redirection entries.
  - `pub fn redirect(irq: u8, vector: u8)` — translates `irq` through
    `overrides`, writes a redirection table entry (low DWORD = vector +
    delivery mode 0 + active high + edge + unmasked, high DWORD = APIC ID 0).
  - Internal: IOAPIC indirect MMIO at `base` (IOREGSEL at `+0x00`, IOWIN
    at `+0x10`).

### `kernel/src/timer.rs`

- `pub fn init(hz: u32)` — calibrates LAPIC frequency with `lapic::calibrate`,
  computes the initial count for `hz`, configures LAPIC timer periodic at
  vector `idt::VEC_LAPIC_TIMER`, registers the handler in the IDT, and unmasks
  it (LAPIC timer is local — no IOAPIC entry needed).
- `pub fn ticks() -> u64` — current tick count (from a `AtomicU64`).
- Handler `extern "x86-interrupt" fn timer_handler(...)`:
  - `TICKS.fetch_add(1, Relaxed)`.
  - `lapic::eoi()`.

### `kernel/src/keyboard.rs`

- `pub fn init()` — installs handler at `idt::VEC_KEYBOARD`, calls
  `ioapic::redirect(1, idt::VEC_KEYBOARD)` so PS/2 IRQ1 fires that vector.
- Handler:
  - Reads scancode from port `0x60`.
  - Logs `ruos: kb scancode=0x{:X}\n` to serial (via a static spin-locked
    Serial, see "Shared serial" below).
  - `lapic::eoi()`.

### `kernel/src/serial.rs` (extension)

The current `Serial` is owned by `kmain`. Interrupt handlers cannot reach it.
Promote logging to a static spin-locked writer:

- `pub static SERIAL: spin::Mutex<Serial> = spin::Mutex::new(Serial::new());`
- A `kprintln!` macro in `main.rs` (or a `print` module) that locks the writer
  and forwards to `core::fmt::Write`.
- `kmain` initializes it once (`SERIAL.lock().init()`).

This is a small, self-contained extension and the only existing-file change in
this milestone besides `main.rs` wiring.

### `kernel/src/main.rs`

- New `RsdpRequest` static in the `.requests` section.
- `kmain` extended per the boot sequence above.

### `Cargo.toml`

Add: `x86_64 = "0.15"`, `acpi = "5"`.

### `Makefile`

Update `HELLO` to `ruos: ticks=` — the presence of this line proves IDT
loaded, APIC enabled, EOI working, `sti` issued, and timer IRQ flowing
end-to-end. The earlier hello/heap/alloc lines are implicit prerequisites.

## Data flow

```
Limine RSDP request -> response (phys addr)
       |
       v
acpi_init::parse -> AcpiInfo { lapic_base, ioapic_base, overrides }
       |
       +--> lapic::init(base) ----------------+
       |                                       |
       +--> ioapic::init(base, overrides)      |
                |                              |
                v                              v
            redirect(IRQ1, VEC_KEYBOARD)   set_timer_periodic(VEC_LAPIC_TIMER, N)
                |                              |
                v                              v
            PS/2 IRQ1 -> handler           periodic IRQ -> handler
                |                              |
                v                              v
         port 0x60 -> serial log       TICKS.fetch_add(1) + eoi()
                |                              |
                v                              v
              eoi()                        kmain busy-wait reads TICKS
```

## Error handling

Every failure path writes a distinct line on serial and halts via `hcf()`:

- `ruos: acpi fail: no rsdp` — Limine did not honor `RsdpRequest`.
- `ruos: acpi fail: no hhdm` — `HHDM_REQUEST` response missing.
- `ruos: acpi fail: parse` — `acpi` crate returned an error.
- `ruos: acpi fail: no lapic` — MADT lacked a LAPIC entry.
- `ruos: acpi fail: no ioapic` — MADT lacked an IOAPIC entry.
- `ruos: timer fail: calibration` — PIT-based calibration produced zero ticks.
- Exception handlers (`#DE`/`#UD`/`#GP`/`#PF`/`#DF`) print their identity and
  the relevant frame fields, then halt.

`#BP` is the only non-halting handler — it logs and returns to let the smoke
test continue.

## Testing

- **Automated (`make run-test`)** asserts the new line `ruos: ticks=` in the
  serial log. Reaching it implies:
  - GDT/TSS/IDT loaded (else triple-fault before any later prints).
  - `pic::disable()` ran (else stray IRQs would never have reached the APIC
    handlers cleanly).
  - ACPI parsing succeeded.
  - LAPIC enabled + EOI works (else the first timer IRQ would block all
    further interrupts).
  - `sti` executed.
  - Timer handler ran multiple times.
- The pre-`ticks=` lines (`hello`, `heap ok`, `alloc box=...`, `idt+apic up`,
  `bp ok rip=...`) all appear in the log too; reviewers can verify by reading
  `build/serial.log`.
- **PS/2 keyboard:** verified manually in `make run` or VirtualBox. Pressing a
  key produces `ruos: kb scancode=0x..` on serial. No headless assertion.
- **Negative paths** (no RSDP, no LAPIC, calibration failure) are exercised by
  code review of the error branches. They are not actively triggered by the
  test environment.

## Open items for the implementation plan

- Pin exact versions of `x86_64` and `acpi` crates that compile against the
  current nightly.
- Confirm the `acpi` crate's `AcpiHandler` trait shape (it has changed across
  4.x and 5.x).
- Decide whether the static `SERIAL` lives in `serial.rs` or a new
  `kernel/src/print.rs` module.
- Confirm IST stack size (16 KiB is the default for `#DF` in `x86_64`-crate
  examples; raise if needed).
- Choose vector numbers for the LAPIC spurious vector (typically `0xFF`) and
  the LAPIC error vector (later).
