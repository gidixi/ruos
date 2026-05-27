# Rust Serial Hello (Limine) — Design Spec

**Date:** 2026-05-27
**Milestone:** "ora sono in Rust" — Steps 1+2+3 of the Rust OS roadmap
(`docs/superpowers/roadmap-rust-os.md`).
**Status:** Approved design, ready for implementation planning.

## Context

The project is pivoting from a C kernel on Pure64 to a **Rust `no_std` kernel booted
by Limine** (see `docs/superpowers/roadmap-rust-os.md`; north star: run
Podman/containers). The existing C code in `x64barebones/` becomes reference material,
not the base.

This spec covers the first executable milestone, bundling roadmap Steps 1-3 because
Step 1 (toolchain) and Step 2 (build) produce nothing testable on their own. The
deliverable is the first commit where "we are in Rust": a kernel that boots under
Limine in QEMU and prints to the serial port, with a panic handler that halts.

Later milestones (heap, IDT/GDT, paging, tasking, user mode, VFS) each get their own
spec → plan → implementation cycle.

## Goals

- Rust nightly toolchain pinned and reproducible, targeting `x86_64-unknown-none`
  with `build-std`.
- A `no_std` / `no_main` kernel crate that Limine loads and enters.
- Serial (COM1, `0x3F8`) output: the kernel prints a recognizable hello line.
- A `#[panic_handler]` that halts the CPU (`cli; hlt` loop).
- A build pipeline (cargo + Makefile) that compiles the kernel, assembles a
  BIOS-bootable Limine ISO with `xorriso`, and runs it in QEMU.
- An automated headless test that boots the ISO and asserts the hello line appears on
  serial.
- Remove the obsolete `x64barebones/Toolchain/` (cross-gcc) directory.

## Non-goals (YAGNI for this milestone)

- No framebuffer output (serial only — far easier to debug).
- No heap / `alloc`, no global allocator (next milestone).
- No interrupts, IDT, GDT, or paging changes.
- No UEFI boot (BIOS only; Limine produces a hybrid ISO so UEFI can be added later
  without redoing this work).
- No global serial writer / `print!` macro infrastructure beyond what the hello needs.
- No deletion of the C kernel/userland (`x64barebones/` stays as reference); only
  `x64barebones/Toolchain/` is removed.

## Repository layout

New top-level `kernel/` crate alongside the `x64barebones/` reference:

```
kernel/
  Cargo.toml                 # crate metadata + deps (limine, uart_16550)
  rust-toolchain.toml        # pinned nightly + components: rust-src, llvm-tools
  .cargo/config.toml         # target x86_64-unknown-none; build-std; linker rustflags
  linker.ld                  # higher-half ELF layout per Limine conventions
  src/main.rs                # no_std/no_main; _start -> kmain; panic handler
  src/serial.rs              # COM1 0x3F8 wrapper over uart_16550
Makefile                     # orchestrator: cargo -> ISO -> QEMU
limine.conf                  # Limine boot entry pointing at the kernel ELF
third_party/limine/          # Limine binary branch (pinned tag); builds host `limine` tool
build/                       # generated: iso_root/, os.iso (gitignored)
.gitignore                   # build/, kernel/target/
```

## Components

### 1. Toolchain configuration

- `kernel/rust-toolchain.toml`: pins a specific `nightly-YYYY-MM-DD` and requests
  components `rust-src` (needed for build-std) and `llvm-tools-preview`.
- `kernel/.cargo/config.toml`:
  - `[build] target = "x86_64-unknown-none"`.
  - `[unstable] build-std = ["core", "alloc", "compiler_builtins"]`.
  - `rustflags` to use the linker script (`-C link-arg=-T linker.ld`) and
    `-C relocation-model=static` as appropriate for a higher-half kernel.
- The exact nightly date and crate versions are pinned during planning.

### 2. Kernel crate (`src/main.rs`)

- `#![no_std]`, `#![no_main]`.
- Declares the Limine base-revision request and uses the `limine` crate's entry
  conventions; defines `#[no_mangle] extern "C" fn _start() -> !` as the ELF entry
  Limine jumps to, which calls `kmain`.
- `kmain` initializes serial and writes the hello line, then enters a `hlt` loop.
- `#[panic_handler]` writes nothing required, executes `cli` then a `hlt` loop
  (never returns).

### 3. Serial (`src/serial.rs`)

- Thin wrapper around `uart_16550::SerialPort` at I/O port `0x3F8`.
- `init()` configures the port; a write function emits a `&str`.
- No global/static writer or locking this milestone — `kmain` owns the port directly.

### 4. Linker script (`linker.ld`)

- Places the kernel in the higher half (base `0xffffffff80000000`) following Limine's
  ELF conventions; defines `.text`, `.rodata`, `.data`, `.bss` and the entry symbol.

### 5. Limine config + binaries

- `limine.conf`: one boot entry naming the kernel ELF and the Limine (native) boot
  protocol.
- `third_party/limine/`: the Limine **binary** branch checked out at a pinned tag.
  Building it (`make -C third_party/limine`) produces the host `limine` deploy tool
  (gcc is available in WSL) and provides the BIOS boot files
  (`limine-bios.sys`, `limine-bios-cd.bin`).

### 6. Makefile (orchestrator)

Targets:
- `build` — `cargo build` in `kernel/` (produces the kernel ELF).
- `limine` — clone (if missing) + build the `third_party/limine` host tool.
- `iso` — assemble `build/iso_root/` (kernel ELF + `limine.conf` + Limine BIOS
  files), run `xorriso` to make a BIOS El-Torito `build/os.iso`, then
  `limine bios-install build/os.iso`.
- `run` — `qemu-system-x86_64 -cdrom build/os.iso -serial stdio` (interactive).
- `run-test` — headless: `qemu-system-x86_64 -cdrom build/os.iso -serial stdio
  -display none -no-reboot`, used by the automated test.
- `clean` — remove `build/` and `kernel/target/`.

## Data flow (build → boot)

```
cargo build (x86_64-unknown-none, build-std, linker.ld)
   -> kernel/target/x86_64-unknown-none/debug/kernel  (ELF, higher-half)
Makefile iso:
   copy kernel ELF + limine.conf + limine BIOS files -> build/iso_root/
   xorriso (El Torito, BIOS) -> build/os.iso
   limine bios-install build/os.iso
QEMU -cdrom build/os.iso  -> Limine -> loads kernel ELF -> _start -> kmain
   -> serial COM1 prints hello -> hlt
```

## Error handling

- A failed `cargo build` stops the pipeline (Makefile dependency).
- ISO assembly, `xorriso`, and `limine bios-install` failures surface as explicit
  Makefile errors — no silent no-ops.
- The panic handler halts the CPU and never returns.

## Testing

All build/run happens in WSL Ubuntu (`/mnt/e/MinimalOS/BasicOperatingSystem`).

- **Build test:** `make iso` succeeds and produces `build/os.iso`.
- **Runtime test:** a script (e.g. `run-test`) runs QEMU headless under a `timeout`
  (the kernel halts and does not exit QEMU on its own), captures serial output, and
  asserts it contains the hello line `MinimalOS-rs: hello serial`. Non-zero/timeout
  exit is expected; the assertion is on the captured serial text.
- **Panic:** verified to compile and (by inspection) halt; an explicit runtime panic
  test is deferred (YAGNI for this milestone).

## Open items for the implementation plan

- Pin the exact `nightly-YYYY-MM-DD` and the `limine` / `uart_16550` crate versions.
- Pin the Limine binary-branch tag.
- Finalize `rustflags`/linker invocation (relocation model, link args) for the
  higher-half ELF.
- Decide the precise hello string and keep the test assertion in sync with it.
