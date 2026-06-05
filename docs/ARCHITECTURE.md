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
runtime** and **concurrency is cooperative async** driven by the timer IRQ. Two
runtimes coexist: **`wasmi`** (an interpreter) runs the `wasm32-wasip1`
coreutils, and **Wasmtime** (no_std, AOT — no JIT) runs precompiled `.cwasm`
modules at near-native speed for the GUI and the WIT/Component-Model bridge.
Every userspace program is a WASM module; the kernel is the host that lends it
memory, files, sockets, a terminal, and a framebuffer through host functions.
A graphical **egui desktop** with a **kernel-side compositor** runs window apps
as separate WASM modules. Everything — kernels, runtimes, and apps — shares one
address space and runs at ring 0.

## Layer cake

```
┌──────────────────────────────────────────────────────────────────┐
│ Userland  .wasm tools (~54): shell, coreutils, nano, rtop, …  +    │
│           GUI: egui desktop + window apps (.cwasm), compositor      │  ring 0,
├──────────────────────────────────────────────────────────────────┤  but
│ Host ABI   wasi_snapshot_preview1 (25) + ruos (33) + ruos_gfx /    │  sandboxed
│            WIT components (framebuffer, input, window mgmt)         │  by the
├──────────────────────────────────────────────────────────────────┤  WASM
│ WASM runtimes   wasmi (interp, .wasm tools) + Wasmtime AOT          │  runtime
│                 (.cwasm GUI/components) · fibers · fuel · limits    │
├──────────────────────────────────────────────────────────────────┤
│ Kernel services VFS · PTY · pipes · proc registry · service mgr ·  │
│                 GUI service (gfx) + compositor · net · storage · SSH│
├──────────────────────────────────────────────────────────────────┤
│ Core kernel     async executor (embassy) · heap (talc) ·           │
│                 frame allocator + paging · IrqMutex · klog          │
├──────────────────────────────────────────────────────────────────┤
│ Arch / drivers  GDT/TSS/IDT · LAPIC/IOAPIC · timer · SMP · PS/2 kbd │
│                 + mouse · PCIe(ECAM) · AHCI · NIC · xHCI · FB · COM1 │
├──────────────────────────────────────────────────────────────────┤
│ Firmware        Limine (UEFI or BIOS) → loads kernel + modules     │
├──────────────────────────────────────────────────────────────────┤
│ Hardware        x86-64 PC (QEMU / VirtualBox / real, 1–N cores)    │
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
| 2 | `mem` | `talc` heap (128 MiB), physical frame allocator + `map/unmap_page`, ACPI tables parsed (MADT, MCFG). |
| 3 | `interrupts` | PIC masked, LAPIC + IOAPIC from MADT, 100 Hz LAPIC timer (calibrated vs ACPI PM timer on real HW), `STI`, then **SMP bring-up** (APs parked in `hlt`). |
| 4 | `pci` | PCIe enumeration over ECAM (from ACPI MCFG). |
| 5 | `devices` | Framebuffer console (font + ANSI), GUI framebuffer geometry captured, PS/2 keyboard **and mouse** (IRQ12). |
| 6 | `fs` | VFS + tmpfs mounted; Limine modules copied into `/bin`, `/etc/init.sh`, etc. |
| 7 | `storage` | AHCI HBA + SATA ports; data partition mounted FAT32 at `/mnt`. |
| 8 | `usb` | xHCI controller, HID boot **keyboard and mouse**, hubs, hot-plug (after FB so logs are visible on real HW; waits for the port to actually enable on real silicon). |
| 9 | `userland` | RNG, networking, service manager, SSH server, then `executor::run()` — **never returns**. |

After phase 9 the framebuffer console is quieted to WARN+ (INFO still flows to
serial and the `dmesg` ring buffer), the async executor takes over, and the
boot shell — spawned from `/etc/init.sh` — gives you a prompt. From there a GUI
`.cwasm` (the egui desktop, or the compositor running window apps) can take over
the framebuffer; see *GUI & compositor* below.

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
  event dispatch), HID boot **keyboard and mouse** drivers, a hub class driver,
  and runtime hot-plug. Devices on root ports or behind hubs all feed the same
  input paths (keystrokes → PTY + GUI, pointer → the mouse queue). USB is
  *polled* (no MSI), so the poll is also pumped from the GUI frame loop — a sync
  GUI owns the cooperative executor and would otherwise starve it. Real-hardware
  quirks handled: BIOS→OS legacy handoff, waiting for *port-enabled* after reset
  (real silicon sets it a few ms after *reset-change*), and a speed-aware
  endpoint-interval encoding so low/full-speed devices are actually polled.
- **Input** — `mouse/` is the PS/2 mouse driver (IRQ12) feeding a shared
  `MouseEvent` queue; PS/2 (`keyboard/`, IRQ1) and USB HID both emit keystrokes
  *and* pointer events. Keystrokes reach the shell via the PTY and the GUI via
  `gfx::push_key` (PS/2 Set 1 scancodes); pointer deltas are folded into an
  absolute cursor by the GUI service.
- **Console & serial** — `console/` is a framebuffer text console (bitmap font,
  anti-aliased blend, `vte` ANSI parser); `serial.rs` is the COM1 driver. A
  monitor + keyboard (+ mouse) or a serial line work interchangeably.
- **Misc** — `rtc.rs` (wall-clock from CMOS), `rng.rs` (see below).

## Core kernel

- **Memory** — `memory/` owns a bitmap **physical frame allocator** built from
  the Limine memory map and a generic paging API (`map_page`/`unmap_page`,
  MMIO ranges, DMA-contiguous frames). The Rust global allocator is **`talc`**
  on a 128 MiB heap, which enables `alloc` (`Vec`/`Box`/`String`/`BTreeMap`).
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

**The second runtime — Wasmtime AOT (`wasm/wt/`).** Graphical and
component-model apps don't run on `wasmi`; they run on **Wasmtime** built
`no_std`, runtime-only (no Cranelift/JIT). The module is **AOT-precompiled** on
the host (`tools/wt-precompile`) to a `.cwasm` and `include_bytes!`'d or staged
at `/bin`, then executed at near-native speed on bare metal — backed by a W^X
executable-memory allocator (`memory/exec.rs`). The shell's command router sends
`.cwasm` to Wasmtime and `.wasm` to `wasmi`. Two extra host surfaces ride on it:
`ruos_gfx` (framebuffer **blit** of guest RGBA8888 + **input events** —
keyboard scancodes and absolute mouse) and a typed **WIT / Component Model**
bridge (`ruos:gui/*`) for the desktop and the compositor's window protocol.

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

## GUI: the kernel↔WASM contract and the compositor

The graphical stack lives in `gfx/` (the kernel-side service) and `wasm/wt/`
(the Wasmtime apps + the compositor); the UI itself is the `ruos-desktop` git
submodule. The whole point of the design is **decoupling**: a GUI app is a
sandboxed WASM module that knows nothing about the kernel — it speaks a small,
fixed ABI, and the kernel is the host that implements it. Two ABI styles coexist.

### Talking to the kernel without coupling

A WASM module cannot call kernel functions directly — there are no syscalls. It
declares **imports** (functions it expects the host to provide) and **exports**
(functions the host may call). The kernel-side `Linker` binds each import name to
a Rust closure that runs in ring 0 with full kernel access, and the guest only
ever sees the function signature. That boundary is the decoupling: either side
can change its internals freely as long as the import/export shapes hold.

ruos uses this boundary at two levels of formality:

- **Raw host modules** (`func_wrap`) — the fast, hand-rolled style used by
  `ruos_gfx` and the compositor's `wm` module. The host registers e.g.
  `wm.commit(ptr, len, w, h)` or `gfx.blit(ptr, len, x, y, w, h)`; pointers are
  offsets into the guest's **own** linear memory, which the host reads/writes
  through `Caller::get_export("memory")` (see `wm.rs::read_guest`/`write_guest`).
  Compound values (a 20-byte input event) are marshalled by hand as
  little-endian fields at fixed offsets. Simple and zero-codegen, but both sides
  must agree on the byte layout by convention.

- **WIT / the Component Model** — the typed, generated style. An interface is
  declared once in a **`.wit`** file (`wit/ruos-gui.wit`, `wit/ruos-bringup.wit`)
  as the *single source of truth*: records, functions, and a `world` of imports
  and exports. For example `ruos:gui` declares a `gfx` interface with
  `get-info -> gfx-info`, `blit(list<u8>, …)`, `poll-event -> option<gfx-event>`,
  and a `power` interface. From that one file:
  - the **guest** generates its import stubs with `wit-bindgen`;
  - the **host** generates a Rust trait with `wasmtime::component::bindgen!`
    (`component.rs`), then just `impl`s it — e.g. `impl ruos::bringup::system::Host
    for BringupHost { fn log(&mut self, msg: String) { … } }`.

  Wasmtime lifts/lowers the values across the boundary (strings, lists,
  `option`, records) — no manual pointer math, no layout to keep in sync. The
  host flow is: `Component::deserialize` the AOT `.cwasm` → `Store` holding the
  host state → component `Linker` → `World::add_to_linker` → `instantiate` →
  call a typed export (`bringup.call_run(&mut store) -> i32`). Because the `.wit`
  is shared and the marshalling is generated, the contract is enforced at
  compile time on both sides — this is the disciplined version of the same
  decoupling the raw modules do by hand, and the direction the real apps move
  toward.

In both cases the guest is still sandboxed by the WASM runtime (its own linear
memory, fuel, resource limits); the host functions are the *only* way out.

### The egui desktop

The desktop UI is plain **egui**, written once in the portable `gui-core` crate.
During development it runs on a PC backend (`winit` + `softbuffer`); for ruos the
same `gui-core` is compiled `wasm32-wasip1`, paired with a thin `ruos-backend`
that implements the `Platform` trait against the `gfx` ABI, and AOT-precompiled
to `gui.cwasm`. Each frame the backend pulls input events (`poll-event`), feeds
them to egui as `RawInput`, lets egui lay out the UI, **rasterises the result on
the CPU with `tiny-skia`** to an RGBA8888 buffer (no GPU — the same raster path
on PC and on device), and `blit`s it. The `gfx` service converts RGBA→panel
layout, does **dirty-rect** updates (only changed regions re-blitted), composites
a software mouse cursor on top, and — because a synchronous GUI owns the
cooperative executor — pumps `usb::poll()` so the polled USB keyboard/mouse keep
delivering events while the GUI runs. Input itself is layout-agnostic: PS/2 and
USB both feed one `MouseEvent` queue (folded into an absolute, screen-clamped
cursor) and emit key events as PS/2 Set 1 scancodes (USB maps HID usage → Set 1),
so the egui side need not know which device produced them.

### The kernel-side compositor

Multi-window is **process-isolated**: every window is a *separate* WASM instance
with its own linear memory (`wasm/wt/wm.rs`). A window app is a "reactor" — it
exports `frame()` and imports the `wm` module (`commit`, `app_id`, `tick`,
`poll_event`, `close`). The compositor owns the screen; the guest only ever draws
into its own surface buffer and drains its own input queue. Per-window state
lives in a `WmState` (id, committed `pixels`, an event `VecDeque`, a
`close_requested` flag); the `Compositor` holds `wins: Vec<Window>` whose **order
is the z-order** (index 0 = bottom, last = top), the focused index, an optional
drag, and a screen back-buffer.

A window app need not be a bare `no_std` reactor. The compositor instantiates
windows on a unified `Store<AppState>` / `Linker<AppState>` where `AppState`
combines the WASI state and the window state, exposed through `HasWasi` /
`HasWindow` accessor traits so `wasi::add_to_linker` and `wm::add_to_linker` both
register onto the *one* linker. That lets a window be a full `wasm32-wasip1`
**std** binary that uses WASI *and* the `wm` surface protocol at once (a
`_initialize` call runs after instantiate for std reactors) — the foundation for
running **egui itself inside a compositor window**, not just fullscreen. The
app-from-shell path keeps its own `Linker<WtState>` unchanged.

One compositor frame (`run_compositor_gate` loop):

1. **Reap** — windows that called `wm.close()` (or finished their lifecycle) are
   dropped; their ids are recycled (`free_ids`).
2. **Route input** — the compositor is the *sole* consumer of `gfx::pop()` in
   this mode. It folds the mouse to an absolute cursor, hit-tests it against
   window footprints, sets focus on a mouse-button-down (**click-to-focus**,
   which also `raise`s the window to the top of `wins`), starts/continues a
   title-bar **drag**, handles the **[X]** close box, and translates the cursor
   to window-local coordinates before pushing the event into **only the focused
   window's** queue. Each app reads its events back via `wm.poll_event`, so a
   window can never see another's input.
3. **Run guests** — `frame_all()` calls every window's `frame()`. The guest
   drains its queue, redraws, and `wm.commit(ptr,len,w,h)`s its surface; the host
   copies those pixels into that window's `WmState.pixels`.
4. **Decorate** — `compose_window` wraps each surface in a **decorated
   footprint**: a focus-coloured title bar, the title text, and an [X] button
   drawn above the committed surface, all in one RGBA buffer.
5. **Composite** — the decorated footprints are painted bottom→top into the
   back-buffer. The screen is split into horizontal **bands** and the
   painter's-algorithm composite of each band (`compose.rs::composite_band`, a
   pure, allocation-free, I/O-free kernel) is **fanned out across the SMP
   compute pool** — APs composite disjoint bands in parallel while the BSP joins.
   This parallel path is verified **byte-identical** to the single-core serial
   path (`run-comp-smp-test`).
6. **Present** — one blit of the back-buffer to the framebuffer, plus the
   software cursor. Clearing each band to the desktop background every frame
   means a moved or closed window leaves no ghost.

A **launcher** strip (taskbar) spawns registered apps (`spawn_app`, capped at
`MAX_WINDOWS`, reusing a shared AOT `Module` so new instances are cheap), and the
lifecycle layer tears down closed windows and recycles their ids and pids. The
single fullscreen egui desktop and this multi-window compositor are two modes of
the same `gfx`/Wasmtime machinery — a window app is just a reactor whose surface
happens to be rendered by egui.

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
| USB (kbd + mouse + hub) | `kernel/src/usb/` |
| Input (PS/2 mouse) | `kernel/src/mouse/`, `kernel/src/keyboard/` |
| VFS / PTY / pipes | `kernel/src/{vfs,pty,pipe}/` |
| Console / serial | `kernel/src/{console,serial.rs}` |
| GUI framebuffer service | `kernel/src/gfx/` |
| WASM runtime (`wasmi`) + host ABI | `kernel/src/wasm/` |
| Wasmtime AOT + WIT + compositor | `kernel/src/wasm/wt/`, `tools/wt-*` |
| egui desktop UI (submodule) | `ruos-desktop/` |
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
