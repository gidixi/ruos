# ruos — a hobby x86-64 OS in Rust

`ruos` is a hobby operating system written in Rust (`no_std`), booted by
[Limine](https://github.com/limine-bootloader/limine). It currently boots in
QEMU (BIOS or UEFI), prints to the serial port (COM1), initializes a kernel
heap on Limine-mapped RAM, and uses `alloc::{Box, Vec, String, BTreeMap}`.

Long-term goal: evolve into an OS capable of running containers
(Podman-style) — driving the design toward real tasking, user mode, a
syscall ABI, and a proper VFS + filesystem.

## Status

Userland = **WebAssembly** (`wasm32-wasi`); the WASM runtime is the sandbox.
No Linux ABI, no CPU ring 3, no preemptive scheduler — concurrency is async
cooperative (timer-IRQ wake). See the
[pivot note](docs/superpowers/roadmap-rust-os.md) for the why.

| Step | Description | Status |
|------|-------------|--------|
| 1–5 | Toolchain, ISO build, `no_std` boot, heap (`talc`), IDT/GDT/APIC/timer/PS2 | ✅ |
| 6 | Physical frame allocator + paging API | ✅ |
| 7 | VFS + tmpfs (in-RAM) | ✅ |
| 8 | Framebuffer console (font, scroll, cursor) | ✅ |
| 9 | Async executor (`no_std`, timer-IRQ wake) | ✅ |
| 10 | WASM runtime (`wasmi`) + WASI Preview 1 | ✅ |
| 11 | Local shell (line editing, PATH, exec `.wasm`, builtins) | ✅ |
| 12 | PTY (pseudo-terminal, line discipline) | ✅ |
| 13 | PCI/PCIe enumeration (ECAM) | ✅ |
| 14 | Networking (virtio-net + e1000, `smoltcp`, RDRAND CSPRNG) | ✅ |
| 15 | AHCI/SATA disk + persistent FAT (`/mnt`) | ✅ |
| 16 | SSH server (`sunset`, ed25519 pubkey, PTY shell + exec) | ✅ |
| 17 | Mouse PS/2 + `rlvgl` GUI + graphics host functions | ⏳ next |

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

## SSH

ruos runs an SSH server on port 22 (`sunset`, ed25519 public-key auth). It
starts at boot once networking and the `/mnt` FAT volume are up. The **host
key** is generated on first boot and persisted to `/mnt/host.key`. Authorized
client keys are read from `/mnt/auth.key` (one ed25519 pubkey, OpenSSH format).

**One-shot automated test** (boots QEMU, forwards `:2222`→guest `:22`, runs an
interactive shell and a non-interactive exec, asserts the output):

```bash
make run-ssh-test
```

**Manual:** put your public key on the disk image, then connect. The Makefile
forwards host `127.0.0.1:2222` to the guest's port 22.

```bash
# 1. Generate a throwaway client key (or reuse build/id_ed25519 from the test).
ssh-keygen -t ed25519 -N '' -f build/id_ed25519 -C ruos

# 2. Copy the *public* key onto the FAT image as /auth.key (8.3 short name).
mcopy -o -i build/disk.img build/id_ed25519.pub ::/auth.key

# 3. Boot ruos with the port-forward (see the run-ssh-test recipe), then:
ssh -p 2222 -i build/id_ed25519 \
    -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    root@127.0.0.1            # interactive shell

ssh -p 2222 -i build/id_ed25519 \
    -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    root@127.0.0.1 pwd        # non-interactive command
```

> OpenSSH refuses private keys that are world-readable. On the Windows DrvFs
> mount the repo copy is `0777`; copy it somewhere native and `chmod 600`
> first (the test script does this automatically).

**Current limits (MVP):** one session at a time, one authorized key, fixed
port 22, pubkey-only (ed25519), username not enforced, exec runs through the
interactive shell (output includes the prompt) with no exit-status, no
window-size / SFTP / forwarding. The server itself runs in ring 0 (the WASM
runtime is the app sandbox, not the SSH server). A release build is required —
debug-profile crypto is too slow (KEX > 60 s).

## Boot on real hardware (USB)

The hybrid ISO can be written directly to a USB stick:

```bash
sudo dd if=build/os.iso of=/dev/sdX bs=4M status=progress conv=fsync
```

Replace `/dev/sdX` with your USB device (e.g. `/dev/sdb`).
**Double-check the device — `dd` will overwrite it entirely.**

Boot from the USB on any x86-64 PC; both BIOS and UEFI firmware are
supported. Output goes to both the **framebuffer console** (Step 8) and COM1
serial, and the PS/2 keyboard drives a local shell — so a monitor + keyboard
is enough; a serial console (USB-to-serial, BMC/IPMI redirect) still mirrors
everything if you need it.

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
