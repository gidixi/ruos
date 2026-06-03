# ruos architecture — hardware, kernel, and OS

A top-to-bottom walkthrough of how ruos works: from the bytes the firmware
loads, through the Rust `no_std` kernel and its drivers, up to the WebAssembly
runtime that runs every userspace tool. Read this after the
[README](../README.md) for the *how it fits together* picture; the README is
the *what's done* picture.

## The one-paragraph thesis

ruos is a single-address-space, ring-0 operating system. There is no CPU
privilege separation (no ring 3, no `SYSCALL`/`SYSRET`, no per-process page
tables) and no preemptive scheduler. Instead, **the sandbox is the WebAssembly
runtime** (`wasmi`) and **concurrency is cooperative async** driven by the timer
IRQ. Every userspace program is a `wasm32-wasip1` module; the kernel is the host
that lends it memory, files, sockets, and a terminal through host functions.
Everything — kernel, runtime, and apps — shares one address space and runs at
ring 0.

## Layer cake

```
┌──────────────────────────────────────────────────────────────────┐
│ Userland  .wasm tools (~54): shell, coreutils, nano, rtop,         │
│           ip/ping/nc/wget, mkdisk/mkboot/install, smptest …        │  ring 0,
├──────────────────────────────────────────────────────────────────┤  but
│ WASI / host ABI   "wasi_snapshot_preview1" (25 fns) + "ruos" (33)  │  sandboxed
├──────────────────────────────────────────────────────────────────┤  by wasmi
│ WASM runtime      wasmi interpreter + fibers + fuel + limits       │
├──────────────────────────────────────────────────────────────────┤
│ Kernel services   VFS · PTY · pipes · proc registry · service mgr  │
│                   net (smoltcp) · storage (AHCI/GPT/FAT32) · SSH    │
├──────────────────────────────────────────────────────────────────┤
│ Core kernel       async executor (embassy) · heap (talc) ·         │
│                   frame allocator + paging · IrqMutex · klog        │
├──────────────────────────────────────────────────────────────────┤
│ Arch / drivers    GDT/TSS/IDT · LAPIC/IOAPIC · timer · SMP ·       │
│                   PS/2 · PCIe(ECAM) · AHCI · NIC · xHCI · FB · COM1 │
├──────────────────────────────────────────────────────────────────┤
│ Firmware          Limine (UEFI or BIOS) → loads kernel + modules   │
├──────────────────────────────────────────────────────────────────┤
│ Hardware          x86-64 PC (QEMU / VirtualBox / real, 1–N cores)  │
└──────────────────────────────────────────────────────────────────┘
```

## Boot: from firmware to shell

Limine (UEFI or BIOS) parses [`limine.conf`](../limine.conf), loads the kernel
ELF plus a list of **modules** (the `.wasm` tools, `init.sh`, and on an
installed disk the kernel/`BOOTX64.EFI`/`limine.conf` themselves), sets up a
higher-half mapping (HHDM), and jumps to `kmain`. The kernel declares what it
needs from Limine as `.requests` in
[`kernel/src/main.rs`](../kernel/src/main.rs): memory map, HHDM offset, RSDP
(ACPI), framebuffer, and the MP (multi-core) response.

`kmain` calibrates a TSC-based boot clock (so early log lines have timestamps
before the LAPIC timer exists), stamps a banner, then runs the boot phases in
strict order — [`kernel/src/boot/mod.rs`](../kernel/src/boot/mod.rs):

| # | Phase | Brings up |
|---|-------|-----------|
| 1 | `arch` | GDT/TSS + IDT loaded (CPU can take exceptions/INT3). |
| 2 | `mem` | `talc` heap (16 MiB), physical frame allocator + `map/unmap_page`, ACPI tables parsed (MADT, MCFG). |
| 3 | `interrupts` | PIC masked, LAPIC + IOAPIC from MADT, 100 Hz LAPIC timer (calibrated vs ACPI PM timer on real HW), `STI`, then **SMP bring-up** (APs parked in `hlt`). |
| 4 | `pci` | PCIe enumeration over ECAM (from ACPI MCFG). |
| 5 | `devices` | Framebuffer console (font + ANSI) and PS/2 keyboard. |
| 6 | `fs` | VFS + tmpfs mounted; Limine modules copied into `/bin`, `/etc/init.sh`, etc. |
| 7 | `storage` | AHCI HBA + SATA ports; data partition mounted FAT32 at `/mnt`. |
| 8 | `usb` | xHCI controller, HID boot keyboard, hubs (after FB so logs are visible on real HW). |
| 9 | `userland` | RNG, networking, service manager, SSH server, then `executor::run()` — **never returns**. |

After phase 9 the framebuffer console is quieted to WARN+ (INFO still flows to
serial and the `dmesg` ring buffer), the async executor takes over, and the
boot shell — spawned from `/etc/init.sh` — gives you a prompt.

## Hardware / driver layer

ruos targets a generic x86-64 PC. What it drives:

- **CPU & SMP** — `cpu/` holds per-CPU GDT/TSS/IDT and the AP trampoline;
  `smp/` reads Limine's MP response, brings each Application Processor online
  with a dense `cpu_id`, and parks it. Core identity is read from the LAPIC ID
  (never `gs:[0]` — a VirtualBox quirk). See *Concurrency* below for what the
  APs actually do.
- **Interrupts** — `gdt.rs`/`idt.rs`/`pic.rs` + `apic/` (LAPIC + IOAPIC). The
  legacy PIC is remapped and masked; all IRQs are routed through the IOAPIC.
- **Timer** — the LAPIC timer fires at 100 Hz and is the single wake source for
  the async executor. On real firmware (which often gates the PIT off) it is
  calibrated against the ACPI PM timer.
- **ACPI** — `acpi_init.rs` parses the RSDP → MADT (CPU + interrupt topology)
  and MCFG (PCIe ECAM base).
- **PCIe** — `pci/` enumerates the bus via ECAM and snapshots every device so
  later drivers (NIC, AHCI, xHCI) can find their BARs.
- **Storage** — `ahci/` is the SATA driver behind the `blockdev.rs`
  `BlockDevice` trait; `gpt.rs` parses/authors GPT, `crc32.rs` covers the GPT
  CRCs, and `vfs/fat32.rs` is a native FAT32 read/write driver (+ `mkfs`/LFN).
- **Networking** — `net/` runs a `smoltcp` TCP/IP stack over two driver
  back-ends: paravirtual **virtio-net** and the Intel **e1000**. DHCP brings
  the interface up.
- **USB** — `usb/` is an xHCI host driver with a USB core (slot registry +
  event dispatch), an HID boot-keyboard driver, a hub class driver, and runtime
  hot-plug. Keyboards on root ports or behind hubs all feed the same input path.
- **Console & serial** — `console/` is a framebuffer text console (bitmap font,
  anti-aliased blend, `vte` ANSI parser); `serial.rs` is the COM1 driver. Both
  PS/2 and USB keystrokes land in one input queue, so a monitor + keyboard or a
  serial line work interchangeably.
- **Misc** — `rtc.rs` (wall-clock from CMOS), `rng.rs` (see below).

## Core kernel

- **Memory** — `memory/` owns a bitmap **physical frame allocator** built from
  the Limine memory map and a generic paging API (`map_page`/`unmap_page`,
  MMIO ranges, DMA-contiguous frames). The Rust global allocator is **`talc`**
  on a 16 MiB heap, which enables `alloc` (`Vec`/`Box`/`String`/`BTreeMap`).
  There are **no** per-process page tables — one address space for everything.
- **Synchronisation** — `sync/` provides `IrqMutex`, an IRQ-safe spinlock that
  disables interrupts while held (so an IRQ handler can't deadlock against a
  lock the interrupted code holds). The cardinal rule learned the hard way:
  never hold a lock across an `.await`, a device transfer, or an enumerate/
  teardown.
- **Async executor** — `executor/` is an `embassy-executor`. It is **single-CPU
  cooperative**: the BSP polls ready tasks, and when none are ready it `hlt`s
  until the next timer tick or an inter-processor wake IPI. This replaces the
  preemptive scheduler the project deliberately dropped.
- **VFS** — `vfs/` defines `FileSystem`/`Inode`/`File` traits with a tmpfs
  (in-RAM) root, the FAT32 driver for `/mnt`, and device files
  (`/dev/console`, `/dev/null`, `/dev/zero`, `/dev/pts/N`). `fd_readdir` is
  wired, so plain `std::fs::read_dir` works from a WASI binary.
- **PTY & pipes** — `pty/` provides master/slave pseudo-terminal pairs with a
  line discipline (cooked mode, `VINTR`/Ctrl-C, echo); `pipe/` is the in-RAM
  pipe backing shell pipelines (`a | b`). The shell runs on a PTY whether local
  or over SSH.
- **Processes & services** — `proc.rs` is a registry of running WASM tasks
  (pid, name, CPU/mem accounting) that `ps`/`rtop`/`kill` read; `service/` is a
  minimal init/service manager (boot respawn of the shell, SSH state).
- **CPU accounting** — `sched/cpustat.rs` keeps per-core busy/idle counters
  using the TSC. Because scheduling is cooperative, "CPU time" is simply the
  cycles a fiber burned between yields; this is what `rtop` graphs.
- **Randomness** — `rng.rs` is a ChaCha20 CSPRNG seeded from the CPU's
  `RDRAND`. This is the entropy source SSH key generation and TLS-style crypto
  depend on.
- **Logging** — `klog.rs` is a ring buffer (readable via `dmesg`) plus
  `kprintln!`; the panic handler in `main.rs` formats once and fans out to
  serial + framebuffer via `try_lock` (never deadlocks), then triggers a
  controlled reboot.

## WASM runtime and the host ABI

`wasm/` is where userland actually runs. Each tool is loaded into a
[`wasmi`](https://github.com/wasmi-labs/wasmi) interpreter instance (pure Rust,
`no_std`). Two import namespaces are linked
([`wasm/host/mod.rs`](../kernel/src/wasm/host/mod.rs)):

- **`wasi_snapshot_preview1`** — 25 standard WASI Preview 1 functions
  (`args_*`, `environ_*`, `fd_read`/`write`/`seek`/`readdir`, `path_*`,
  `clock_time_get`, `random_get`, `poll_oneoff`, `proc_exit`). This is what lets
  an unmodified `wasm32-wasip1` `std` binary run.
- **`ruos`** — 33 custom host functions for things WASI doesn't cover:
  terminal control (`tcgetattr`/`tcsetattr`, `poll_stdin`), system info
  (`meminfo`, `cpustat`, `proc_stat` for `ps`/`rtop`), sockets, SMP benchmarking
  (`smp_bench`), and disk authoring/install (`mkdisk`, `mkboot`, `install`).

Host-call mechanics:

- **Fibers** — a blocking WASI call (e.g. `fd_read` on a PTY with no input)
  can't busy-wait, so each WASM task runs on a **fiber** (`wasm/fiber.rs`,
  `suspend.rs`). The fiber suspends, the executor runs other work, and a wake
  (keypress, timer, socket event) resumes it exactly where it left off. From the
  guest's point of view the call simply blocked.
- **Fuel metering** — each execution slice gets a `FUEL_PER_SLICE =
  2_000_000_000` instruction budget. A pure compute loop with no host calls
  exhausts it and is killed (exit 137); I/O-bound tasks refuel on every host
  call and run indefinitely. This bounds a runaway guest without preemption.
- **Resource limits** — a `wasmi::ResourceLimiter` caps linear-memory pages and
  table elements per instance.
- **Capability-scoped paths** — host path functions reject any path that
  escapes the task's declared root (no `../` past `/`).
- **One audited memory accessor** — every host function that touches guest
  linear memory goes through `wasm/host/mem.rs::check_bounds` (fuzz-tested);
  there are no raw guest-memory reads/writes elsewhere.

`exec_queue.rs` and `pipeline.rs` schedule tool invocations (including pipe
stages, which run as concurrent cooperative fibers); `ssh_spawn.rs` wires an
SSH channel to a fresh shell on a PTY.

## Userland / OS layer

- **Shell** — `user/shell`: line editing (←/→/⌫, tab path completion), PATH
  lookup in `/bin` then `/mnt/bin`, `cmd1 | cmd2` pipelines, builtins (`cd`,
  `pwd`, `source`, power commands), and `exec` of any `/bin/*.wasm` (or
  `/mnt/bin/*.wasm` on an installed disk, loaded on-demand from the FAT).
- **Tools** — ~54 `wasm32-wasip1` crates under `user/`, built to `user-bin/`.
  On the live ISO they are Limine modules at `/bin`; on an installed SSD the
  command tools live on the data partition (`/mnt/bin`) and load on-demand,
  while only the bootstrap (shell, init, network/SSH service) stays on the slim
  ESP. (The live medium has no readable filesystem after boot — no USB
  mass-storage driver — so it must carry every tool as a module.)
- **init** — `/etc/init.sh` (a tiny greeter by default; the test build swaps in
  `smoke.sh`, the full assertion battery).
- **SSH** — `ssh/` is the `sunset` library bridged to the kernel: ed25519 host
  key, password (PBKDF2) + ed25519 pubkey auth, an interactive PTY shell, and
  non-interactive exec. It starts at boot and runs even disklessly (ephemeral
  RAM host key). The SSH server itself runs in ring 0; the app it spawns is the
  sandboxed part.
- **Self-install** — from a running shell, `install` (no arg) lists the SATA
  disks; `install <n>` authors that disk (GPT + FAT32) and writes a bootable
  system — a **slim ESP** (kernel + shell + init + network/SSH, via a reduced
  `limine-ssd.conf`) plus the command tools on the **data partition**
  (`/mnt/bin`, loaded on-demand). The disk boots ruos standalone under UEFI. A
  `/mnt` guard refuses to wipe the running system (and prevents a re-install
  loop on the SSD's own reboot, where `/mnt` is already mounted). The FAT writer
  authors long filenames (LFN) and the mounted driver reads them back, so the
  tools keep their real names on disk.

## Concurrency model (and what SMP does)

The default model is **cooperative async on one core**: the BSP runs the
executor, tasks yield at `.await` points, and the timer IRQ wakes sleepers.
There is no preemption, so a task that never yields owns the CPU until fuel
runs out.

SMP is layered on top without disturbing that: the **other cores are a
compute-offload pool** (`smp/pool.rs`). The BSP `submit`s pure-CPU jobs
(`fn(&[u8]) -> u64`, no I/O, no captures) into a fixed slot array; APs in their
`hlt`-idle worker loop `take` and run them in parallel, and the BSP collects
results with `poll_done`. `smptest` shows a 2–3× speedup. The async executor and
all I/O stay on the BSP — APs never touch the VFS, runtime, or device locks.

## Lifecycle of one command

```
keypress (PS/2 or USB HID)            firmware/driver IRQ
  → input queue → PTY line discipline (cooked, echo, ^C)
  → shell fiber reads a line, resolves /bin/<cmd>.wasm (then /mnt/bin) via VFS
  → exec_queue loads the module into a wasmi instance
       linker installs wasi_snapshot_preview1 + ruos host fns
       ResourceLimiter + fuel armed
  → fiber runs; host calls hit kernel services (VFS, net, term…)
       blocking call → fiber suspends → executor runs others
       timer/socket/key event → fiber resumes
  → guest calls proc_exit (or fuel exhausts → killed)
  → fiber torn down, proc registry entry removed, prompt returns
```

## Where things live (source map)

| Area | Path |
|------|------|
| Entry, Limine requests, panic | `kernel/src/main.rs` |
| Boot phases | `kernel/src/boot/` |
| Memory (frames, paging, heap) | `kernel/src/memory/` |
| Interrupts (GDT/IDT/APIC/PIC) | `kernel/src/{gdt,idt,pic}.rs`, `apic/` |
| SMP (cores + job pool) | `kernel/src/{cpu,smp,sched}/` |
| PCIe | `kernel/src/pci/` |
| Storage (SATA, GPT, FAT32) | `kernel/src/{ahci,gpt,disk,crc32}.rs`, `kernel/src/vfs/fat32.rs` |
| Networking | `kernel/src/net/` |
| USB | `kernel/src/usb/` |
| VFS / PTY / pipes | `kernel/src/{vfs,pty,pipe}/` |
| Console / serial | `kernel/src/{console,serial.rs}` |
| WASM runtime + host ABI | `kernel/src/wasm/` |
| SSH | `kernel/src/ssh/` |
| Async executor | `kernel/src/executor/` |
| Userspace tools | `user/` → `user-bin/` |

## Further reading

- [README](../README.md) — status, build, test, run, SSH, install.
- [`docs/superpowers/roadmap-rust-os.md`](superpowers/roadmap-rust-os.md) — the
  step-by-step roadmap and the 2026 pivot rationale.
- [`docs/superpowers/specs/`](superpowers/specs/) and
  [`docs/superpowers/plans/`](superpowers/plans/) — per-subsystem design specs
  and implementation plans.
- [`CHANGELOG/`](../CHANGELOG/) — one entry per change, in order.
