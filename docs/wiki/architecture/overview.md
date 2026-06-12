# Architettura — panoramica

> **Stato:** bozza
> **Aggiornato:** 2026-06-10
> **Fonti:** `kernel/src/boot/mod.rs`, `kernel/src/main.rs`

ruOS è un OS x86-64 in Rust `no_std`, bootato da **Limine**, in cui **tutto lo
userland è WebAssembly**: le app sono moduli `.wasm`/`.cwasm` e il runtime WASM è
la sandbox (niente ring 3, niente ELF Linux, niente thread preemptivi veri e propri,
ma concorrenza async cooperativa con *epoch-based watchdog* per interrompere i guest).

## Tesi in un paragrafo

Single-address-space, ring 0. Niente separazione di privilegio CPU, niente
`SYSCALL`/`SYSRET`, niente page table per-processo. La **sandbox è il runtime
WASM** e la **concorrenza è cooperativa async** (timer IRQ 100 Hz come wake
source). Due runtime: **wasmi** (interprete, tool `.wasm` CLI) e **Wasmtime AOT**
(`no_std`, `.cwasm` precompilati per GUI e Component Model, con epoch scheduling). Un **desktop egui**
con **compositor kernel-side** fa girare le app finestra come moduli WASM separati.

## Layer cake

```
┌──────────────────────────────────────────────────────────────┐
│ Userland  .wasm (~58): shell, coreutils, nano, gzip…        │
│           GUI/TUI: egui desktop, rtop, window apps (.cwasm) │  ring 0,
├──────────────────────────────────────────────────────────────┤  sandboxed
│ Host ABI   wasi_snapshot_preview1 (25) + ruos (33)           │  by WASM
│            wm (20) + sys (4) + term (5) + ruos_gfx (6)       │  runtime
│            WIT components (ruos:gui/*, ruos:tui/*, etc.)     │
├──────────────────────────────────────────────────────────────┤
│ WASM runtimes   wasmi (interp) + Wasmtime AOT (no_std)       │
│                 fibers · fuel metering · resource limits     │
├──────────────────────────────────────────────────────────────┤
│ Kernel services VFS · PTY · pipes · proc registry · svc mgr │
│                 GUI (gfx) + compositor · net · storage · SSH │
├──────────────────────────────────────────────────────────────┤
│ Core kernel     async executor (embassy) · heap (talc)       │
│                 frame allocator + paging · IrqMutex · klog   │
├──────────────────────────────────────────────────────────────┤
│ Arch / drivers  GDT/TSS/IDT · LAPIC/IOAPIC · timer · SMP    │
│                 PS/2 kbd+mouse · PCIe · AHCI · NIC · xHCI   │
├──────────────────────────────────────────────────────────────┤
│ Firmware        Limine (UEFI o BIOS) → carica kernel + moduli│
├──────────────────────────────────────────────────────────────┤
│ Hardware        x86-64 PC (QEMU / VirtualBox / reale, 1–N)  │
└──────────────────────────────────────────────────────────────┘
```

## Boot a fasi

Il boot procede per 10 fasi sequenziali (`kernel/src/boot/mod.rs`):

```
arch → mem → interrupts (+SMP) → pci → devices (framebuffer + PS/2)
     → fs (VFS/tmpfs) → storage (AHCI/FAT32 /mnt) → usb (xHCI)
     → media_bin (liveCD overlay) → userland (RNG, net, SSH, executor)
```

Ogni fase porta online un sottosistema. Dopo la fase 10 l'executor async prende
il controllo e il sistema è a regime: shell su PTY, SSH in ascolto, desktop GUI.

Dettagli: [Boot a fasi](../components/boot-phases.md).

## Due runtime WASM

- **wasmi** — interprete `no_std`, esegue i tool `.wasm` (wasm32-wasip1).
  Lento ma sicuro. Fuel metering a 2G istruzioni/slice.
- **Wasmtime AOT** `no_std` — esegue i `.cwasm` precompilati (GUI/compositor +
  Component Model) a velocità quasi-nativa, memoria W^X. Niente JIT.

La shell decide: `.wasm` → wasmi, `.cwasm` → Wasmtime.

Dettagli: [Runtime WASM](../components/wasm-runtime.md).

## Concorrenza: executor + SMP

Il modello è **cooperative async su un core, con compute offload SMP**:

- **BSP**: executor `embassy`, tutti i task, tutto l'I/O.
- **AP**: pool di job puri CPU (compositing parallelo, benchmark). Niente I/O,
  niente lock, niente VFS.
- **Scheduling misto**: la concorrenza è cooperativa (timer IRQ a 100 Hz sveglia
  i task sleeping), ma per i task `Wasmtime` è attivo l'**epoch-based scheduling**.
  Agisce da *watchdog*: se un task è bloccato in un loop CPU-bound, l'epoch scatta
  e il task viene "trappato" (killed) o interrotto per rilasciare la CPU all'executor.

Dettagli: [SMP / executor](../components/smp-executor.md).

## I sottosistemi

| Sottosistema | Pagina wiki | Cosa fa |
|---|---|---|
| Boot | [Boot a fasi](../components/boot-phases.md) | 10 fasi, da GDT a executor |
| Runtime WASM | [Runtime WASM](../components/wasm-runtime.md) | wasmi + Wasmtime, fibers, fuel, host ABI |
| VFS / Storage | [VFS / Storage](../components/vfs-storage.md) | tmpfs, FAT32, AHCI, GPT, disk authoring |
| Input | [Input](../components/input.md) | PS/2 + USB HID, coda mouse, fold_mouse |
| Networking + SSH | [Networking + SSH](../components/networking-ssh.md) | smoltcp, NIC driver, DHCP, TCP, SSH, Wi-Fi |
| SMP / Executor | [SMP / executor](../components/smp-executor.md) | embassy, compute pool, CPU accounting |
| Compositor | [Compositor](../components/compositor.md) | Multi-window, input routing, compositing SMP |

## Ciclo di vita di un comando

```
keypress (PS/2 o USB HID)                  IRQ hardware
  → coda input → PTY line discipline (cooked, echo, ^C)
  → shell fiber legge una riga, risolve /bin/<cmd>.wasm via VFS
  → exec_queue carica il modulo in una istanza wasmi
       linker installa wasi_snapshot_preview1 + ruos host fns
       ResourceLimiter + fuel armati
  → fiber esegue; host call raggiungono i servizi kernel
       call bloccante → fiber sospende → executor esegue altro
       timer/socket/key → fiber riprende
  → guest chiama proc_exit (o fuel esausto → killed)
  → fiber distrutta, proc entry rimossa, prompt ritorna
```

## Dove va tutto (source map)

| Area | Path |
|------|------|
| Entry, Limine, panic | `kernel/src/main.rs` |
| Boot phases | `kernel/src/boot/` |
| Memory | `kernel/src/memory/` |
| Interrupts | `kernel/src/{gdt,idt,pic}.rs`, `apic/` |
| SMP | `kernel/src/{cpu,smp,sched}/` |
| PCIe | `kernel/src/pci/` |
| Storage | `kernel/src/{ahci,gpt,disk,blockdev,crc32}.rs`, `vfs/fat32.rs` |
| Networking | `kernel/src/net/` |
| USB | `kernel/src/usb/` |
| Input | `kernel/src/{keyboard,mouse}/` |
| VFS / PTY / pipes | `kernel/src/{vfs,pty,pipe}/` |
| Console / serial | `kernel/src/{console,serial.rs}` |
| GUI / compositor | `kernel/src/gfx/`, `kernel/src/wasm/wt/wm.rs` |
| WASM runtimes | `kernel/src/wasm/`, `kernel/src/wasm/wt/` |
| SSH | `kernel/src/ssh/` |
| Executor | `kernel/src/executor/` |
| Userspace tools | `user/` → `user-bin/` |
| egui desktop | `ruos-desktop/` (git submodule) |

## Vedi anche

- [README](../../../README.md) — status, build, test, run, SSH, install
- [ARCHITECTURE.md](../../ARCHITECTURE.md) — walkthrough dettagliato top-to-bottom
- [API reference](../../api/README.md) — manuale host functions
- [Indice della wiki](../README.md)
