# ruos — a hobby x86-64 OS in Rust

`ruos` is a hobby operating system written in Rust (`no_std`), booted by
[Limine](https://github.com/limine-bootloader/limine). It currently boots in
QEMU (BIOS or UEFI), prints to the serial port (COM1), initializes a kernel
heap on Limine-mapped RAM, and uses `alloc::{Box, Vec, String, BTreeMap}`.

Long-term goal: evolve into an OS capable of running containers
(Podman-style) — driving the design toward real tasking, user mode, a
syscall ABI, and a proper VFS + filesystem.

## Status

| Step | Description | Status |
|------|-------------|--------|
| 1 | Rust nightly toolchain + `x86_64-unknown-none` + `build-std` | ✅ |
| 2 | Cargo + `Makefile` orchestrator (kernel ELF → Limine ISO) | ✅ |
| 3 | `no_std` kernel that boots from Limine and writes to serial | ✅ |
| 4 | Global allocator (`talc`) + heap on Limine memmap / HHDM | ✅ |
| 5 | IDT, GDT, interrupt handlers (PIT, PS/2 keyboard) | ⏳ next |
| 6 | Physical frame allocator + paging API (Rust-side) | — |
| 7 | Tasking (TCB, context switch, scheduler) | — |
| 8 | User mode + syscall ABI | — |
| 9 | VFS + filesystem (tmpfs, FAT, block driver) | — |

Detailed roadmap: [`docs/superpowers/roadmap-rust-os.md`](docs/superpowers/roadmap-rust-os.md).
Per-step design specs and implementation plans live under
[`docs/superpowers/specs/`](docs/superpowers/specs/) and
[`docs/superpowers/plans/`](docs/superpowers/plans/). A flat changelog of every
change is in [`CHANGELOG/`](CHANGELOG/).

## Prerequisites

Build is Linux-native (POSIX tools, GCC, GNU make). On Windows, use **WSL2
with Ubuntu** (or any other Linux VM).

System packages (Ubuntu / Debian):

```bash
sudo apt update
sudo apt install -y build-essential curl git xorriso qemu-system-x86
```

Rust toolchain (via [`rustup`](https://rustup.rs)):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- \
    -y --default-toolchain nightly --component rust-src
. "$HOME/.cargo/env"
rustup component add llvm-tools-preview
```

`kernel/rust-toolchain.toml` pins the exact nightly used for development
(currently `nightly-2026-05-26`); `rustup` syncs to it automatically on the
first `cargo build`.

## Build

```bash
git clone https://github.com/gidixi/ruos.git
cd ruos
make iso
```

The first `make iso` clones the Limine binary branch (`v11.4.1-binary`) into
`third_party/limine/`, builds its host tool, then assembles
`build/os.iso` — a hybrid BIOS (El Torito) + UEFI ISO.

## Test (automated, headless)

```bash
make run-test
```

Boots the ISO in QEMU headless under a 30 s timeout, captures the serial
output, and asserts the full-success line. Expected serial:

```
ruos: hello serial
ruos: heap ok base=0xFFFF800000100000 size=4194304
ruos: alloc box=0xCAFEBABE vec=[0, 1, 2, 3, 4]
```

Final make output:

```
TEST_PASS
```

The kernel halts after the smoke test; QEMU is killed by `timeout`, which is
expected. The test verdict is based on the captured serial, not on QEMU's
exit code.

## Run (interactive)

```bash
make run
```

Launches QEMU with a display window; serial is mirrored to the host
terminal.

## Boot on real hardware (USB)

The hybrid ISO can be written directly to a USB stick:

```bash
sudo dd if=build/os.iso of=/dev/sdX bs=4M status=progress conv=fsync
```

Replace `/dev/sdX` with your USB device (e.g. `/dev/sdb`).
**Double-check the device — `dd` will overwrite it entirely.**

Boot from the USB on any x86-64 PC; both BIOS and UEFI firmware are
supported. The kernel currently writes only to COM1, so on real hardware you
need a serial console (USB-to-serial cable, BMC/IPMI redirect, etc.) to see
the output. Framebuffer text output is on the roadmap after Step 5.

## Repository layout

```
kernel/                 # Rust no_std kernel crate
  src/
    main.rs             # entry, Limine requests, kmain
    serial.rs           # COM1 driver (uart_16550)
    memory.rs           # global allocator (talc) + init_heap
  linker.ld             # higher-half ELF layout (ENTRY(kmain))
  .cargo/config.toml    # target, build-std, rustflags
  rust-toolchain.toml   # pinned nightly + components
Makefile                # build / iso / run / run-test / clean
limine.conf             # Limine boot entry → /boot/kernel
docs/superpowers/       # roadmap, design specs, implementation plans
CHANGELOG/              # one Markdown entry per change (NN-yy-mm-dd-slug.md)
LICENSE                 # GNU GPL v3.0
```

Build artifacts live under `build/` and `kernel/target/` (gitignored).
The Limine binary branch is vendored to `third_party/limine/` on first
build (gitignored).

## Reference / history

The project started as a fork of an x86-64 hobby OS in C (Pure64 + a full C
kernel: bitmap frame allocator from BIOS E820, 4 KiB paging, buddy heap,
RTL8139 driver, simple shell). After completing the C memory manager, the
project pivoted to a Rust + Limine rewrite. The legacy C tree was removed
from `main` but survives in git history up to commit `c1d2a81`; see
[`docs/superpowers/plans/2026-05-27-memory-manager.md`](docs/superpowers/plans/2026-05-27-memory-manager.md)
for the C design and implementation history.

## License

GNU General Public License v3.0. See [`LICENSE`](LICENSE).
