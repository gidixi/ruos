# ruos — a hobby x86-64 OS in Rust

`ruos` is a hobby operating system written in Rust (`no_std`), booted by
[Limine](https://github.com/limine-bootloader/limine). It boots in QEMU
(BIOS or UEFI), VirtualBox, and on real hardware via USB; runs `.wasm`
userspace tools (coreutils, network tools, an editor); ships a TCP/IP
stack with virtio-net and e1000 drivers; mounts an AHCI/SATA disk as FAT32
at `/mnt`; and exposes an interactive shell over **SSH** (port 22, ed25519
host key, password + pubkey auth).

**North star** (pivot 2026-05-28): execute `.wasm` apps (WASI), GUI via
`rlvgl`, remote access via SSH. Userland = WebAssembly (`wasm32-wasip1`);
the WASM runtime is the sandbox. No Linux ABI, no CPU ring 3, no preemptive
scheduler — concurrency is async cooperative (timer-IRQ wake). See the
[pivot note](docs/superpowers/roadmap-rust-os.md) for the why.

## Status

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
| 16 | SSH server (`sunset`, ed25519 host key, pubkey **+ password** auth, PTY shell + exec, runs disklessly) | ✅ |
| 17 | Mouse PS/2 + `rlvgl` GUI + graphics host functions | ⏳ next |

WASI compatibility is growing incrementally alongside the roadmap: as of
the latest work, `fd_readdir` is exported, so `std::fs::read_dir` and
`walkdir`-style crates work from a plain `wasm32-wasip1` `std` binary (no
custom `ruos.*` bindings needed). The legacy `ruos.readdir` host fn stays
for the existing tools.

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
sudo apt install -y build-essential curl git xorriso qemu-system-x86 \
                    mtools dosfstools         # FAT image (/mnt) generation
# Optional, only for the SSH password test:
sudo apt install -y sshpass
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

The Makefile has separate targets for the boot smoke battery, SSH pubkey,
SSH password (with `/mnt/passwd`), and SSH password from a diskless boot
(compile-time default fallback):

```bash
make run-test                  # boot + DHCP + AHCI + FAT + shell + pipe
make run-ssh-test              # SSH client w/ ed25519 pubkey, exec + interactive
make run-passwd-test           # SSH password from /mnt/passwd (sshpass)
make run-passwd-diskless-test  # SSH password fallback, no -drive
```

`run-test` swaps in `user-bin/smoke.sh` as `/etc/init.sh` so the boot
exercises every `.wasm` tool, FAT R/W, ping, and a shell pipe. Each test
captures the serial log under `build/`, greps for marker lines (e.g. `dhcp
bound ip=10.0.2.15`, `mnt mounted FAT`, `auth ok user=root`) and prints
`TEST_PASS_*` on success. The kernel halts after smoke; QEMU is killed by
`timeout`, which is expected — the verdict is based on the captured serial.

## Run (interactive)

```bash
make run
```

Launches QEMU with a display window; serial is mirrored to the host
terminal.

## SSH

ruos runs an SSH server on port 22 (`sunset`, ed25519 host key). It starts
at boot — no disk required: if `/mnt` is unavailable the host key is
generated ephemerally in RAM (fingerprint changes every boot, fine for
demos). Two auth methods, both enabled by default:

| Method | Source | Notes |
|---|---|---|
| **Password** | `/mnt/passwd` (PBKDF2-HMAC-SHA256), or compile-time default `RUOS_DEFAULT_PASSWORD` (defaults to `ruos`) | Out-of-the-box: ISO-only boot, no extra setup. |
| **Pubkey** (ed25519) | `/mnt/auth.key` (OpenSSH format, one key per line) | Requires the disk image, but stronger and scriptable. |

**Out-of-the-box (no disk attached, no key setup):**

```bash
ssh root@<vm-ip>            # password: ruos
```

Works in QEMU `make run` (host-fwd `127.0.0.1:2222`) and VirtualBox bridged
networking. Change the default at build time:

```bash
make iso RUOS_PASSWORD=hunter2
```

**Seed a stronger PBKDF2 hash on disk (overrides the built-in default):**

```bash
make passwd-on-disk RUOS_PASSWORD=hunter2
# regenerates /passwd in build/disk.img; surfaces in the guest as /mnt/passwd
```

**Pubkey:**

```bash
# 1. Generate a throwaway client key (or reuse build/id_ed25519 from the test).
ssh-keygen -t ed25519 -N '' -f build/id_ed25519 -C ruos

# 2. Copy the *public* key onto the FAT image as /auth.key (8.3 short name).
mcopy -o -i build/disk.img build/id_ed25519.pub ::/auth.key

# 3. Boot with port-forwarding (see make run / make run-ssh-test), then:
ssh -p 2222 -i build/id_ed25519 \
    -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    root@127.0.0.1
```

> OpenSSH refuses private keys that are world-readable. On the Windows DrvFs
> mount the repo copy is `0777`; copy it somewhere native and `chmod 600`
> first (the test script does this automatically).

**Current limits (MVP):** one session at a time, one authorized key, fixed
port 22, single password (no per-user accounts), username not enforced,
exec runs through the interactive shell (output includes the prompt) with
no exit-status, no window-size / SFTP / forwarding. The server itself runs
in ring 0 (the WASM runtime is the app sandbox, not the SSH server). A
release build is required — debug-profile crypto is too slow (KEX > 60 s).
The compile-time default password lives in plain-text in the kernel
binary and is convenience for demos, not a security mechanism — set
`/mnt/passwd` (or your own `RUOS_DEFAULT_PASSWORD` at build time) for any
real use.

## Security model

ruos runs everything — kernel, WASM runtime, and all userspace tools — in
**ring 0** (kernel privilege). The WASM runtime (`wasmi`) is the app sandbox,
not the CPU's hardware privilege rings. There is no ring 3, no `SYSCALL`/`SYSRET`
setup, and no per-process page tables.

### Hardening present (Fase A — blast-radius)

The following defences are implemented and tested:

| Layer | Mechanism |
|---|---|
| Host-boundary safety | One audited guest-memory accessor (`kernel/src/wasm/host/mem.rs::check_bounds`). Every host fn that reads or writes WASM linear memory goes through this function — no raw `.read`/`.write` elsewhere. Fuzz-tested with adversarial cases (negative ptr/len, overflow, boundary) and an exhaustive small-grid never-panics run. |
| Fuel metering | Each WASM task is given a 2 000 000 000 instruction budget. A pure compute loop with no host calls exhausts the budget and is killed (exit 137, logged as `wasm: task killed (fuel exhausted)`). I/O-bound tasks refuel on every host call and run indefinitely. |
| Per-task resource limits | `wasmi::ResourceLimiter` caps memory pages and table elements per instance. |
| Capability-scoped paths | Host path functions reject paths that escape the task's declared root (no `../` traversal past `/`). |
| Non-deadlocking panic + reset | Kernel panics print a backtrace, drop locks, and trigger a controlled reboot — they do not deadlock the machine. |

### Honest ceiling

This is **defence-in-depth in ring 0**, not hardware isolation.

- A memory-safety bug inside the `wasmi` interpreter itself, or in the
  kernel's own `unsafe` blocks, is still fatal to the entire system — there
  is no separate address space to contain it.
- Only a separate address space **plus** CPU privilege level (e.g. ring 3 with
  per-process page tables) could contain such a bug. That architecture is
  explicitly out of scope per the project's WASM-as-sandbox thesis and would
  require adding a preemptive scheduler, SYSCALL/SYSRET stubs, and GDT ring-3
  segments.
- The compile-time default password lives in plain text in the kernel binary
  and is a convenience for demos only — it is not a security mechanism.
- **Do not over-claim:** ruos is a hobby OS with a ring-0 WASM sandbox that
  meaningfully reduces the blast radius of buggy app code. It is not a
  hardened multi-tenant platform.

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
    boot/               # phased boot sequence (arch → mem → fs → user)
    apic/, gdt.rs,      # interrupt setup (LAPIC/IOAPIC, GDT/TSS, IDT)
      idt.rs, pic.rs
    serial.rs, klog.rs, # COM1 driver + ring-buffer log + kprintln!
      kprint.rs
    memory/             # frame allocator + paging (talc + map/unmap_page)
    acpi_init.rs        # ACPI MADT/ECAM parse
    pci/                # PCIe enumeration (ECAM)
    net/                # smoltcp stack + virtio-net + e1000 drivers
    ahci/, blockdev.rs  # SATA driver + block device trait
    vfs/                # tmpfs + FAT32 + /dev/{console,null,zero,pts/N}
    pty/                # 4 master/slave pairs + line discipline
    pipe/               # in-RAM pipe for shell pipelines
    console/            # framebuffer console (font, AA blend, vte ANSI)
    wasm/               # wasmi runtime, WASI shim, exec_queue, pipeline
    ssh/                # sunset bridge + hostkey + authkeys + password
    executor/           # embassy-executor + per-task wakers
    proc.rs, modules.rs,# proc registry, Limine module mounts, RNG, RTC
      rng.rs, rtc.rs,
      timer.rs
  linker.ld             # higher-half ELF layout (ENTRY(kmain))
  build.rs              # git SHA + build date + env tracking
  .cargo/config.toml    # target, build-std, rustflags
  rust-toolchain.toml   # pinned nightly + components
user/                   # WASI userspace crates (one per .wasm tool)
user-bin/               # built .wasm artefacts (checked in) + init/smoke.sh
Makefile                # iso / run / run-test / run-ssh-test / ...
limine.conf             # Limine boot entry + module list
tests/                  # bash drivers for SSH / pipe / passwd tests
docs/superpowers/       # roadmap, design specs, implementation plans
CHANGELOG/              # one Markdown entry per change (NN-yy-mm-dd-slug.md)
LICENSE                 # GNU GPL v3.0
```

Build artifacts live under `build/` and `kernel/target/` (gitignored).
The Limine binary branch is cloned to `third_party/limine/` on first build
(gitignored); the `sunset` SSH library is vendored to `third_party/sunset/`
and checked in.

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
