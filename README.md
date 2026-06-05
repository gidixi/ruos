```
            ___   ____
 _ __ _   _/ _ \ / ___|
| '__| | | | | | \___ \
| |  | |_| | |_| |___) |
|_|   \__,_|\___/ |____/
        a WASM-first x86-64 OS in Rust
```

# ruos — a WASM-first x86-64 OS in Rust

`ruos` is a from-scratch, WASM-first x86-64 operating system written in Rust
(`no_std`), booted by
[Limine](https://github.com/limine-bootloader/limine). It boots in QEMU
(BIOS or UEFI), VirtualBox, and on real hardware via USB; runs `.wasm`
userspace tools (coreutils, network tools, an editor, an `htop`-style
monitor); renders a graphical **egui desktop** with a kernel-side
**compositor** (multiple WASM window apps, drag/raise/close, a launcher);
ships a TCP/IP stack with virtio-net and e1000 drivers; mounts an AHCI/SATA
disk as FAT32 at `/mnt`; drives USB keyboards **and mice** over xHCI (hubs +
hot-plug); offloads pure-CPU work across cores (SMP); can **install itself
onto an internal SATA SSD** and boot standalone; and exposes an interactive
shell over **SSH** (port 22, ed25519 host key, password + pubkey auth).

**North star** (pivot 2026-05-28): execute `.wasm` apps (WASI), a GUI
(**egui**, rasterised on-device with `tiny-skia`), remote access via SSH.
Userland = WebAssembly (`wasm32-wasip1`); the WASM runtime is the sandbox. No
Linux ABI, no CPU ring 3, no preemptive scheduler — concurrency is async
cooperative (timer-IRQ wake). See the
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
| 17 | Mouse (PS/2 **+ USB HID**) + framebuffer GUI service (`ruos_gfx` ABI) + graphics host functions | ✅ |
| 18 | **egui desktop** (rasterised with `tiny-skia`, run as a Wasmtime-AOT `.cwasm`) + kernel↔WASM bridge (WIT / Component Model) | ✅ |
| 19 | **Kernel-side compositor / window manager** — multiple WASM window apps, input routing + click-to-focus, decorations + drag/raise/close, SMP-parallel compositing, a launcher + app lifecycle | ✅ |

> The GUI replaces the originally-planned `rlvgl` (see the pivot note): apps are
> standard egui, rasterised on-device with `tiny-skia` and blitted through the
> `ruos_gfx` host ABI, so the same UI code runs on a PC backend during
> development and on ruos unchanged.

### Built alongside (beyond the numbered roadmap)

Several subsystems were built alongside the roadmap — all merged to `main` and
verified in QEMU + VirtualBox (USB input, the GUI, and the installer also
confirmed on real hardware):

| Subsystem | What | Status |
|---|---|---|
| **SMP** | Multi-core bring-up (Limine MP, per-CPU GDT/TSS/IDT) + a cooperative compute-offload pool — APs run pure-CPU kernel jobs in parallel while the BSP async executor is untouched (`smptest` shows 2–3× speedup). The compositor also fans out per-band compositing across the APs. Still no preemptive scheduler. | ✅ |
| **USB** | xHCI host driver + HID boot **keyboard and mouse**, USB **hubs**, and runtime **hot-plug** — attach/detach on a root port or behind hubs and it drives the shell and the GUI. Real-HW fixes: PED-after-reset wait, speed-aware endpoint interval, and pumping `usb::poll()` from the GUI loop (USB is polled, not IRQ-driven, so a sync GUI would otherwise starve it). | ✅ |
| **Wasmtime AOT** | A second WASM runtime alongside `wasmi`: Wasmtime in `no_std`, runtime-only (no JIT), running **AOT-precompiled `.cwasm`** at near-native speed. Powers the GUI (the egui desktop) and the WIT / Component Model kernel↔WASM bridge. The shell's `.cwasm` router picks it; `wasmi` still runs the `.wasm` coreutils. | ✅ |
| **GUI / desktop** | A framebuffer GUI service (`ruos_gfx` ABI: RGBA8888 blit + input events) hosting an **egui** desktop (developed against a PC backend in the `ruos-desktop` submodule, rasterised with `tiny-skia`), plus a **kernel-side compositor** that runs each window as a separate WASM app, routes input with click-to-focus, draws decorations, handles drag/raise/close, composites window bands in parallel across cores, and offers a launcher. PS/2 and USB keyboard + mouse both drive it. | ✅ |
| **`rtop`** | `htop`-style full-screen monitor (per-core CPU%, memory, uptime, process table) — `ratatui` on `wasm32-wasip1`, timer-driven auto-refresh, Ctrl-C foreground kill. | ✅ |
| **SSD self-install** | `install` lists the SATA disks (`install <n>` picks one), authors it (GPT + FAT32), and writes a bootable system: a **slim ESP** (kernel + shell + init + network/SSH) plus the command tools on the **data partition**. The SSD boots ruos standalone under UEFI, mounts its data partition, and the shell loads tools **on-demand** from `/mnt/bin`. A `/mnt` guard refuses to wipe the running system. | ✅ |

WASI compatibility is growing incrementally alongside the roadmap: as of
the latest work, `fd_readdir` is exported, so `std::fs::read_dir` and
`walkdir`-style crates work from a plain `wasm32-wasip1` `std` binary (no
custom `ruos.*` bindings needed). The legacy `ruos.readdir` host fn stays
for the existing tools.

**How it all works:** a top-to-bottom walkthrough of the hardware, kernel,
and userland — boot flow, drivers, the WASM runtime, and the host ABI — is in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

Detailed roadmap: [`docs/superpowers/roadmap-rust-os.md`](docs/superpowers/roadmap-rust-os.md).
Per-step design specs and implementation plans live under
[`docs/superpowers/specs/`](docs/superpowers/specs/) and
[`docs/superpowers/plans/`](docs/superpowers/plans/). A flat changelog of every
change is in [`CHANGELOG/`](CHANGELOG/).

## Userspace tools

Every tool is a `wasm32-wasip1` binary in `user/`. On the live ISO/USB they are
Limine modules mounted under `/bin`; on an **installed SSD** only the bootstrap
(kernel, shell, init, network/SSH) sits on the slim ESP, and the command tools
live on the data partition (`/mnt/bin`) — the shell loads them on-demand. Add a
new one by appending its crate name to `BIN_TOOLS` in the `Makefile`.

| Group | Tools |
|---|---|
| Files & text | `ls cat echo cp mv rm mkdir rmdir touch find du df wc head tail sort uniq cut tr tee grep diff which clear` |
| Editor | `nano` |
| System & process | `ps kill pkill uname whoami id uptime free dmesg lscpu service rtop` |
| Network | `ip ifconfig ping nc wget lspci` |
| Disk & install | `mkdisk mkboot install` |
| SMP & misc | `smptest spinloop readdirtest date` |

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
rustup target add wasm32-wasip1 wasm32-unknown-unknown   # WASI tools/UI + compositor windows
```

`kernel/rust-toolchain.toml` pins the exact nightly used for development
(currently `nightly-2026-05-26`); `rustup` syncs to it automatically on the
first `cargo build`.

## Build

```bash
git clone https://github.com/gidixi/ruos.git
cd ruos
git submodule update --init --recursive   # ruos-desktop (egui UI → gui.cwasm)
make iso
```

The first `make iso` clones the Limine binary branch (`v11.4.1-binary`) into
`third_party/limine/`, builds its host tool, compiles the WASM tools and the
egui desktop, AOT-precompiles the GUI/compositor `.cwasm`, then assembles
`build/os.iso` — a hybrid BIOS (El Torito) + UEFI ISO.

## Test (automated, headless)

The Makefile has a target per subsystem. The core boot battery:

```bash
make run-test                  # boot smoke: PCI + xHCI USB + DHCP + AHCI + FAT + readdir + rtop + shell pipe
make run-test-e1000            # run-test on the Intel e1000 NIC path
```

SSH, PTY, and WASM-sandbox behaviour:

```bash
make run-ssh-test              # SSH client w/ ed25519 pubkey, exec + interactive
make run-passwd-test           # SSH password from /mnt/passwd (sshpass)
make run-passwd-diskless-test  # SSH password fallback, no -drive (diskless)
make run-pipe-test             # shell pipeline `a | b` over a PTY
make run-fuel-test             # WASM fuel metering kills a runaway compute loop
make run-ctrlc-test            # Ctrl-C kills the foreground app, prompt returns
make run-ssh-idle-test         # idle SSH session survives the PTY watchdog
```

SMP, the monitor, and USB:

```bash
make run-smp-test              # SMP bring-up: every AP comes online
make run-smp2-test             # SMP compute pool: parallel speedup over APs
make run-rtop-test             # rtop over SSH: auto-refresh + clean quit
make run-usb-key-test          # USB keyboard (root port) types into the shell
make run-usb-hub-test          # USB keyboard behind a hub enumerates + types
make run-usb-hotplug-test      # USB keyboard added / removed at runtime
make run-comp-smp-test         # compositor: SMP-parallel vs serial compositing is byte-identical
```

The kernel also runs in-boot self-tests under `make iso CARGO_FEATURES=boot-checks`
(e.g. the HID mouse decode, the USB-usage→scancode map, and the window-manager
logic), and a serial-less real-hardware USB triage build under
`make iso CARGO_FEATURES=usb-probe` (dumps the device/port/slot table and halts).

Disk authoring and boot-from-SSD (`run-m2b2-test` is the installer capstone —
it boots the authored SSD standalone under OVMF/UEFI):

```bash
make run-gpt-test              # GPT-partitioned SATA disk parsed + /mnt mounted
make run-m2a-test              # `mkdisk` authors a GPT+FAT32 disk (host-verified)
make run-m2b1-test             # boot tree copied onto the ESP (LFN, byte-identity)
make run-m2b2-test             # install to SSD, then boot standalone from it
```

`run-test` swaps in `user-bin/smoke.sh` as `/etc/init.sh` so the boot
exercises every `.wasm` tool, FAT R/W, ping, a shell pipe, `rtop`, and the
USB keyboard. Each test captures the serial log under `build/`, greps for
marker lines (e.g. `dhcp bound ip=10.0.2.15`, `mnt mounted FAT`, `usb  xhci
up`, `rtop: uptime=`, `auth ok user=root`) and prints `TEST_PASS_*` on
success. The kernel halts after smoke; QEMU is killed by `timeout`, which is
expected — the verdict is based on the captured serial.

## Run (interactive)

```bash
make run
```

Launches QEMU with a display window; serial is mirrored to the host
terminal.

## Install to an internal disk (SSD)

ruos can author a blank SATA disk and copy its own boot tree onto it, so the
machine then boots ruos from the SSD with no USB stick attached. From a
running ruos shell (serial, framebuffer, or SSH):

```
install            # auto-targets the first non-boot SATA disk
install 1          # or name the AHCI port explicitly
```

`install` writes a GPT (protective MBR + primary/backup headers), formats an
EFI System Partition (FAT32) plus a data partition, and copies the kernel,
`BOOTX64.EFI`, `limine.conf`, and every `.wasm` tool onto the ESP with long
filenames. On the next UEFI boot the firmware runs
`/EFI/BOOT/BOOTX64.EFI` → Limine → the kernel, and the data partition mounts
at `/mnt`. Proven end-to-end under OVMF (`make run-m2b2-test`).

A safety guard refuses to run while `/mnt` is mounted, so it can neither wipe
the disk it booted from nor re-install in a loop. The lower-level steps are
also exposed as standalone tools: `mkdisk` (author GPT + FAT32 only) and
`mkboot` (copy the boot tree onto an already-authored ESP).

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

ruos runs everything — kernel, WASM runtimes, and all userspace tools — in
**ring 0** (kernel privilege). The WASM runtime is the app sandbox, not the
CPU's hardware privilege rings: `wasmi` interprets the `.wasm` coreutils and
Wasmtime (no_std, AOT) runs the precompiled `.cwasm` GUI/compositor apps, with
guest code held to its linear memory + WASI/`ruos_gfx` host imports. There is no
ring 3, no `SYSCALL`/`SYSRET` setup, and no per-process page tables.

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
- **Do not over-claim:** ruos is a single-address-space, ring-0 OS whose WASM
  sandbox meaningfully reduces the blast radius of buggy app code. It is not a
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
serial, and a **PS/2 or USB keyboard and mouse** drive the local shell and the
GUI — so a monitor + keyboard (+ optional mouse) is enough; a serial console
(USB-to-serial, BMC/IPMI redirect) still mirrors the logs if you need it. The
boot path is hardened for real firmware: the clock no longer polls the
(often-gated) PIT, the LAPIC timer is calibrated against the ACPI PM timer,
xHCI takes ownership from the BIOS via the USB legacy-support handoff, and the
xHCI port-reset path waits for the controller to actually enable the port
(real silicon raises *port-enabled* a few milliseconds after *reset-change*,
unlike QEMU) so external keyboards and mice enumerate.

## Repository layout

```
kernel/                 # Rust no_std kernel crate
  src/
    main.rs             # entry, Limine requests, kmain
    boot/               # phased boot sequence (arch → mem → fs → user)
    apic/, gdt.rs,      # interrupt setup (LAPIC/IOAPIC, GDT/TSS, IDT)
      idt.rs, pic.rs
    cpu/, smp/          # per-CPU GDT/TSS/IDT + AP trampoline; SMP bring-up + job pool
    sched/              # TSC-based per-core CPU accounting (feeds rtop)
    serial.rs, klog.rs, # COM1 driver + ring-buffer log + kprintln!
      kprint.rs
    memory/             # frame allocator + paging (talc + map/unmap_page)
    acpi_init.rs        # ACPI MADT/ECAM parse
    pci/                # PCIe enumeration (ECAM)
    net/                # smoltcp stack + virtio-net + e1000 drivers
    usb/                # xHCI driver + HID keyboard & mouse + hub + hot-plug
    ahci/, blockdev.rs  # SATA driver + block device trait
    gpt.rs, disk.rs,    # GPT parse/author + FAT32 mkfs + boot-tree copy (install)
      crc32.rs
    vfs/                # tmpfs + FAT32 + /dev/{console,null,zero,pts/N}
    pty/                # 4 master/slave pairs + line discipline
    pipe/               # in-RAM pipe for shell pipelines
    service/, sync/     # minimal service manager; IrqMutex (IRQ-safe lock)
    console/            # framebuffer console (font, AA blend, vte ANSI)
    gfx/                # framebuffer GUI service (ruos_gfx ABI, mouse fold, cursor)
    mouse/              # PS/2 mouse (IRQ12) → shared MouseEvent queue
    wasm/               # wasmi runtime, WASI shim, exec_queue, pipeline
      wt/               #   Wasmtime AOT runtime + WIT/Component Model + compositor/WM
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
ruos-desktop/           # git submodule: portable egui UI (gui-core) + ruos backend
                        #   → AOT-compiled to gui.cwasm and run on the GUI service
tools/                  # host + guest helpers: wt-precompile (AOT), wt-reactor* (compositor windows)
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
and checked in. The egui UI lives in the **`ruos-desktop` git submodule**
(`git submodule update --init --recursive` after clone — `make iso` needs it to
build `gui.cwasm`).

## License

GNU General Public License v3.0. See [`LICENSE`](LICENSE).
