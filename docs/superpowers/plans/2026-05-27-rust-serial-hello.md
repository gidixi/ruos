# Rust Serial Hello (Limine) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Boot a Rust `no_std` kernel via Limine (BIOS, ISO) in QEMU and print a hello line over serial COM1, with a panic handler that halts.

**Architecture:** A `kernel/` Rust crate targeting `x86_64-unknown-none` with `build-std`, linked higher-half by a custom `linker.ld` per Limine conventions. The kernel uses the native Limine boot protocol (the `limine` crate) and writes to COM1 via `uart_16550`. A root `Makefile` orchestrates cargo build → Limine ISO assembly with `xorriso` → QEMU. Verification is a headless QEMU boot whose serial output is asserted to contain the hello line.

**Tech Stack:** Rust nightly, `build-std`, target `x86_64-unknown-none`, crates `limine` + `uart_16550`, Limine bootloader (binary branch), `xorriso`, QEMU. All build/run inside WSL Ubuntu.

---

## Key facts (environment & conventions)

- **All build/run runs in WSL Ubuntu** as root. Repo at `/mnt/e/MinimalOS/BasicOperatingSystem`. Wrap commands:
  ```
  wsl -d Ubuntu -u root -e bash -c '<cmd>'
  ```
  rustup installs to `/root/.cargo`; prefix cargo/rustc commands with `source $HOME/.cargo/env &&`.
- **Edit files** with Edit/Write on Windows paths (e.g. `E:\MinimalOS\BasicOperatingSystem\kernel\...`). Build/run only via WSL. Git in the normal shell. Branch: `feature/rust-serial-hello`. Do not push, do not skip hooks.
- **Spec:** `docs/superpowers/specs/2026-05-27-rust-serial-hello-design.md`. **Roadmap:** `docs/superpowers/roadmap-rust-os.md`.
- **Entry point convention:** the kernel ELF entry is `kmain` (set via `ENTRY(kmain)` in `linker.ld`); there is no separate `_start` (the spec mentioned `_start → kmain`; the canonical Limine-rs approach makes `kmain` the entry directly — this is the resolution of that wording).
- **Verification style:** kernel bring-up has no unit tests; each task's "test" is a concrete build/boot check with expected output. This is intentional and matches the domain.
- **Integration risk (read before starting):** exact crate APIs (`limine`, `uart_16550`) and the Limine config-file syntax depend on the pinned versions. The code below uses the widely-used canonical pattern (limine-rs barebones). If `cargo build` reports an API mismatch against the actually-resolved crate version, adapt minimally to that version's API and report what changed — do NOT redesign. Likewise match `limine.conf` syntax to the cloned Limine binary tag.

## File structure

- `kernel/Cargo.toml` — crate metadata + deps (`limine`, `uart_16550`), panic=abort profiles.
- `kernel/rust-toolchain.toml` — nightly channel + components `rust-src`, `llvm-tools-preview`.
- `kernel/.cargo/config.toml` — target, `build-std`, linker rustflags.
- `kernel/linker.ld` — higher-half ELF layout, `ENTRY(kmain)`, `.requests` sections.
- `kernel/src/main.rs` — `no_std`/`no_main`, Limine base-revision requests, `kmain`, panic handler.
- `kernel/src/serial.rs` — COM1 `0x3F8` wrapper over `uart_16550`.
- `limine.conf` — Limine boot entry pointing at `/boot/kernel`.
- `Makefile` — root orchestrator (`build`, `limine`, `iso`, `run`, `run-test`, `clean`).
- `third_party/limine/` — Limine binary branch (cloned by the Makefile; gitignored).
- `.gitignore` — append `build/`, `kernel/target/`, `third_party/limine/`.
- Removed: `x64barebones/Toolchain/`.

---

## Task 1: WSL Rust toolchain + remove cross-gcc Toolchain

**Files:**
- Delete: `x64barebones/Toolchain/` (whole directory)
- Create: `CHANGELOG/10-26-05-27-rust-toolchain.md`

- [ ] **Step 1: Install OS packages needed for the build**

Run:
```
wsl -d Ubuntu -u root -e bash -c 'export DEBIAN_FRONTEND=noninteractive; apt-get update -qq && apt-get install -y -qq curl xorriso && echo PKG_OK'
```
Expected: ends with `PKG_OK`. (gcc/make/qemu are already installed from the C work.)

- [ ] **Step 2: Install rustup with nightly + components (non-interactive)**

Run:
```
wsl -d Ubuntu -u root -e bash -c 'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly --component rust-src llvm-tools-preview && source $HOME/.cargo/env && rustc --version && cargo --version'
```
Expected: prints `rustc 1.XX.0-nightly (...)` and `cargo 1.XX.0-nightly (...)`.

- [ ] **Step 3: Verify the bare-metal target is usable via build-std**

Run:
```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && rustc --print target-list | grep -x x86_64-unknown-none && echo TARGET_OK'
```
Expected: prints `x86_64-unknown-none` then `TARGET_OK`. (We build `core`/`alloc` for it via `build-std`; no `rustup target add` needed.)

- [ ] **Step 4: Remove the obsolete cross-gcc Toolchain directory**

```bash
git rm -r x64barebones/Toolchain
```
(This removes the ModulePacker/cross-gcc tree no longer used by the Rust build. The C kernel reference no longer needs it; the legacy C image build will not be run going forward.)

- [ ] **Step 5: Write the changelog entry**

Create `CHANGELOG/10-26-05-27-rust-toolchain.md`:
```markdown
# 10 — Toolchain Rust nightly in WSL + rimozione Toolchain/

**Data:** 2026-05-27

## Cosa
- Installati in WSL Ubuntu: curl, xorriso, e rustup con toolchain nightly +
  componenti rust-src e llvm-tools-preview.
- Verificato il target x86_64-unknown-none (usato via build-std).
- Rimossa la cartella x64barebones/Toolchain/ (cross-gcc ModulePacker, non più usata).

## Perché
Step 1 della roadmap Rust: predisporre il toolchain per il kernel no_std e
liberarsi del cross-gcc.

## File toccati
- x64barebones/Toolchain/ (rimossa)
- CHANGELOG/10-26-05-27-rust-toolchain.md
```

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "build(rust): install nightly toolchain in WSL, remove cross-gcc Toolchain"
```

---

## Task 2: Kernel crate (no_std skeleton + serial, builds to ELF)

**Files:**
- Create: `kernel/Cargo.toml`
- Create: `kernel/rust-toolchain.toml`
- Create: `kernel/.cargo/config.toml`
- Create: `kernel/linker.ld`
- Create: `kernel/src/serial.rs`
- Create: `kernel/src/main.rs`
- Modify: `.gitignore`
- Create: `CHANGELOG/11-26-05-27-kernel-crate.md`

- [ ] **Step 1: Create `kernel/Cargo.toml`**

```toml
[package]
name = "kernel"
version = "0.1.0"
edition = "2021"

[dependencies]
limine = "0.4"
uart_16550 = "0.3"

[profile.dev]
panic = "abort"

[profile.release]
panic = "abort"
```
(If `cargo build` later reports `limine = "0.4"` does not exist, use the latest published `0.x` and record the version in the changelog; the request/BaseRevision API used below is stable across recent 0.x.)

- [ ] **Step 2: Create `kernel/rust-toolchain.toml`**

```toml
[toolchain]
channel = "nightly"
components = ["rust-src", "llvm-tools-preview"]
```
(Channel is left as rolling `nightly` for now. After the first green build in Task 3, pin the exact `nightly-YYYY-MM-DD` reported by `rustc --version` for reproducibility and note it in that task's changelog.)

- [ ] **Step 3: Create `kernel/.cargo/config.toml`**

```toml
[build]
target = "x86_64-unknown-none"

[unstable]
build-std = ["core", "compiler_builtins", "alloc"]
build-std-features = ["compiler-builtins-mem"]

[target.x86_64-unknown-none]
rustflags = ["-C", "link-arg=-Tlinker.ld", "-C", "relocation-model=static"]
```
(`alloc` is built now though unused this milestone, so the heap milestone needs no config change. `compiler-builtins-mem` provides `memcpy`/`memset`/etc. for `no_std`.)

- [ ] **Step 4: Create `kernel/linker.ld`**

```ld
OUTPUT_FORMAT(elf64-x86-64)
ENTRY(kmain)

PHDRS
{
    requests PT_LOAD;
    text     PT_LOAD;
    rodata   PT_LOAD;
    data     PT_LOAD;
}

SECTIONS
{
    . = 0xffffffff80000000;

    .requests : {
        KEEP(*(.requests_start_marker))
        KEEP(*(.requests))
        KEEP(*(.requests_end_marker))
    } :requests

    . = ALIGN(CONSTANT(MAXPAGESIZE));

    .text : {
        *(.text .text.*)
    } :text

    . = ALIGN(CONSTANT(MAXPAGESIZE));

    .rodata : {
        *(.rodata .rodata.*)
    } :rodata

    . = ALIGN(CONSTANT(MAXPAGESIZE));

    .data : {
        *(.data .data.*)
    } :data

    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    } :data

    /DISCARD/ : {
        *(.eh_frame*)
        *(.note .note.*)
    }
}
```

- [ ] **Step 5: Create `kernel/src/serial.rs`**

```rust
use core::fmt;
use uart_16550::SerialPort;

/// Minimal COM1 (0x3F8) writer over uart_16550.
pub struct Serial {
    port: SerialPort,
}

impl Serial {
    pub fn new() -> Self {
        // Safety: 0x3F8 is the standard COM1 I/O port base.
        Serial { port: unsafe { SerialPort::new(0x3F8) } }
    }

    pub fn init(&mut self) {
        self.port.init();
    }
}

impl fmt::Write for Serial {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            self.port.send(b);
        }
        Ok(())
    }
}
```

- [ ] **Step 6: Create `kernel/src/main.rs`**

```rust
#![no_std]
#![no_main]

mod serial;

use core::fmt::Write;
use core::panic::PanicInfo;
use limine::BaseRevision;
use limine::request::{RequestsEndMarker, RequestsStartMarker};

/// Tell Limine which base revision we support.
#[used]
#[link_section = ".requests"]
static BASE_REVISION: BaseRevision = BaseRevision::new();

#[used]
#[link_section = ".requests_start_marker"]
static _START_MARKER: RequestsStartMarker = RequestsStartMarker::new();

#[used]
#[link_section = ".requests_end_marker"]
static _END_MARKER: RequestsEndMarker = RequestsEndMarker::new();

#[no_mangle]
unsafe extern "C" fn kmain() -> ! {
    assert!(BASE_REVISION.is_supported());

    let mut serial = serial::Serial::new();
    serial.init();
    let _ = serial.write_str("MinimalOS-rs: hello serial\n");

    hcf();
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    hcf();
}

/// Halt and catch fire: disable interrupts and halt forever.
fn hcf() -> ! {
    loop {
        unsafe { core::arch::asm!("cli; hlt") };
    }
}
```
(If the resolved `limine` crate exposes `BaseRevision`/markers under different paths, adjust the `use` lines to match that version and note it. The `.requests*` section names must match `linker.ld`.)

- [ ] **Step 7: Append build artifacts to `.gitignore`**

Append these lines to the existing root `.gitignore` (do not remove existing entries):
```
# Rust OS build artifacts
/build/
/kernel/target/
/third_party/limine/
```

- [ ] **Step 8: Build the kernel to an ELF**

Run:
```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem/kernel && cargo build 2>&1 | tail -20 && ls -la target/x86_64-unknown-none/debug/kernel'
```
Expected: `cargo build` finishes (`Compiling kernel`, `Finished`), and `ls` shows the `kernel` ELF exists. If the link fails on `-Tlinker.ld` not found, change the rustflag to an absolute path or confirm cargo's CWD is the crate root.

- [ ] **Step 9: Confirm it is a higher-half ELF**

Run:
```
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem/kernel && readelf -h target/x86_64-unknown-none/debug/kernel | grep -E "Type|Entry"'
```
Expected: `Type: EXEC` (or `DYN`) and an `Entry point address` of `0xffffffff800xxxxx`.

- [ ] **Step 10: Write the changelog entry**

Create `CHANGELOG/11-26-05-27-kernel-crate.md`:
```markdown
# 11 — Crate kernel Rust no_std (skeleton + seriale)

**Data:** 2026-05-27

## Cosa
- Creato il crate kernel/ (Cargo.toml, rust-toolchain.toml, .cargo/config.toml,
  linker.ld higher-half con ENTRY(kmain), src/main.rs no_std/no_main con richieste
  Limine + panic handler che halta, src/serial.rs su COM1 via uart_16550).
- build-std (core/alloc/compiler_builtins) su target x86_64-unknown-none.
- .gitignore aggiornato per build/ e target/.

## Perché
Step 2-3 della roadmap: kernel Rust che compila a un ELF higher-half pronto per Limine.

## File toccati
- kernel/Cargo.toml, kernel/rust-toolchain.toml, kernel/.cargo/config.toml
- kernel/linker.ld, kernel/src/main.rs, kernel/src/serial.rs
- .gitignore
- CHANGELOG/11-26-05-27-kernel-crate.md
```

- [ ] **Step 11: Commit**

```bash
git add kernel .gitignore CHANGELOG/11-26-05-27-kernel-crate.md
git commit -m "feat(rust): no_std kernel crate with Limine requests and serial"
```

---

## Task 3: Limine ISO + Makefile + headless boot test

**Files:**
- Create: `limine.conf`
- Create: `Makefile`
- Create: `CHANGELOG/12-26-05-27-limine-iso-boot.md`
- (Generated, gitignored: `third_party/limine/`, `build/`)

- [ ] **Step 1: Create `limine.conf`**

```
timeout: 0

/MinimalOS-rs
    protocol: limine
    path: boot():/boot/kernel
```
(This is the Limine v8 config syntax. If the cloned Limine tag uses the older `limine.cfg`/`KERNEL_PATH=` syntax, match it instead and note the deviation.)

- [ ] **Step 2: Create the root `Makefile`**

```make
KERNEL    := kernel/target/x86_64-unknown-none/debug/kernel
LIMINE    := third_party/limine
ISO_ROOT  := build/iso_root
ISO       := build/os.iso
HELLO     := MinimalOS-rs: hello serial

.PHONY: all build limine iso run run-test clean

all: iso

build:
	source $$HOME/.cargo/env && cd kernel && cargo build

limine:
	@if [ ! -d $(LIMINE) ]; then \
		git clone https://github.com/limine-bootloader/limine.git \
			--branch=v8.x-binary --depth=1 $(LIMINE); \
	fi
	$(MAKE) -C $(LIMINE)

iso: build limine
	rm -rf $(ISO_ROOT)
	mkdir -p $(ISO_ROOT)/boot/limine $(ISO_ROOT)/EFI/BOOT
	cp $(KERNEL) $(ISO_ROOT)/boot/kernel
	cp limine.conf $(ISO_ROOT)/boot/limine/
	cp $(LIMINE)/limine-bios.sys $(LIMINE)/limine-bios-cd.bin \
	   $(LIMINE)/limine-uefi-cd.bin $(ISO_ROOT)/boot/limine/
	cp $(LIMINE)/BOOTX64.EFI $(ISO_ROOT)/EFI/BOOT/
	xorriso -as mkisofs -b boot/limine/limine-bios-cd.bin \
		-no-emul-boot -boot-load-size 4 -boot-info-table \
		--efi-boot boot/limine/limine-uefi-cd.bin \
		-efi-boot-part --efi-boot-image --protective-msdos-label \
		$(ISO_ROOT) -o $(ISO)
	$(LIMINE)/limine bios-install $(ISO)

run: iso
	qemu-system-x86_64 -cdrom $(ISO) -serial stdio -m 512

run-test: iso
	@echo "--- serial (timeout 30s) ---"
	-timeout 30 qemu-system-x86_64 -cdrom $(ISO) -serial stdio -display none -no-reboot -m 512

clean:
	rm -rf build kernel/target
```
(Recipes must use TAB indentation. `$$HOME` escapes the make `$` so the shell expands `$HOME`.)

- [ ] **Step 3: Build the ISO**

Run:
```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso 2>&1 | tail -25 && ls -la build/os.iso'
```
Expected: clones+builds Limine the first time, assembles the ISO, `limine bios-install` succeeds, and `ls` shows `build/os.iso`. If `make -C third_party/limine` fails, check that `gcc`/`make` are present (they are) and read the error.

- [ ] **Step 4: Boot headless and verify the serial hello (the test)**

Run:
```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test 2>&1 | tee /tmp/serial.log; grep -q "MinimalOS-rs: hello serial" /tmp/serial.log && echo TEST_PASS || echo TEST_FAIL'
```
Expected: the serial log contains `MinimalOS-rs: hello serial` and the command prints `TEST_PASS`. (QEMU is killed by `timeout` after 30s because the kernel halts; that is expected.)

If `TEST_FAIL`: inspect `/tmp/serial.log`. Common causes — Limine config syntax mismatch (Step 1), wrong kernel path in `limine.conf`, `.requests` sections not matching `linker.ld`, or a panic before serial init. Report findings rather than guessing blindly.

- [ ] **Step 5: Pin the nightly for reproducibility**

Capture the active nightly and pin it:
```
wsl -d Ubuntu -u root -e bash -c 'source $HOME/.cargo/env && rustc --version'
```
Take the `(... YYYY-MM-DD)` date and edit `kernel/rust-toolchain.toml` `channel = "nightly"` → `channel = "nightly-YYYY-MM-DD"`. Re-run Step 4's build to confirm it still produces `TEST_PASS`.

- [ ] **Step 6: Write the changelog entry**

Create `CHANGELOG/12-26-05-27-limine-iso-boot.md`:
```markdown
# 12 — Limine ISO + Makefile + boot seriale "hello"

**Data:** 2026-05-27

## Cosa
- limine.conf (protocollo Limine, kernel /boot/kernel).
- Makefile orchestratore: build cargo, clone+build Limine binario, assemblaggio
  iso_root, xorriso (BIOS El Torito) + limine bios-install, run/run-test/clean.
- Boot headless in QEMU verifica la stringa seriale "MinimalOS-rs: hello serial".
- Pinnato il nightly esatto in rust-toolchain.toml per riproducibilità.

## Perché
Completa il milestone "ora sono in Rust": kernel Rust che bota da Limine e parla
sulla seriale, primo artefatto eseguibile della riscrittura.

## File toccati
- limine.conf
- Makefile
- kernel/rust-toolchain.toml
- CHANGELOG/12-26-05-27-limine-iso-boot.md
```

- [ ] **Step 7: Commit**

```bash
git add limine.conf Makefile kernel/rust-toolchain.toml CHANGELOG/12-26-05-27-limine-iso-boot.md
git commit -m "feat(rust): Limine ISO build and headless serial boot test"
```

---

## Notes for the implementer

- **WSL wrapper everywhere:** every build/run command goes through `wsl -d Ubuntu -u root -e bash -c '...'` and sources `$HOME/.cargo/env` for cargo/rustc.
- **Adapt, don't redesign:** if a crate API or Limine config syntax differs from the canonical code here (version drift), make the minimal adaptation to compile/boot and report exactly what you changed. The architecture (Limine native protocol, higher-half ELF, serial-first) stays.
- **First boot is the risky step.** If the kernel triple-faults or Limine can't find the kernel, the serial log is empty — treat that as the signal to check `limine.conf` path, the `.requests` markers, and the linker entry, in that order.
- **Legacy C build is no longer run.** Removing `x64barebones/Toolchain/` breaks the old C image build by design; the C sources remain only as reference.
```
