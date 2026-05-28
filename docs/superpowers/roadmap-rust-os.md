# Roadmap — Riscrittura OS in Rust (no_std) + Limine

**Data:** 2026-05-27
**North star:** far girare **Podman/container**. Guida le scelte: tasking thread-style,
user mode, syscall, VFS + filesystem reale, e (a lungo termine) compatibilità Linux.

## Cambio di direzione

Il vecchio kernel C su Pure64 (`x64barebones/`, gestore memoria completo, RTL8139,
shell) è stato **rimosso dal working tree**: vive solo come riferimento storico
in git history fino al commit `c1d2a81` (plan/spec a `docs/superpowers/...`).
Si riusa la conoscenza (parsing E820, bitmap frame allocator, buddy heap, paging
4 KiB, COM1 0x3F8), non il codice. Il nuovo OS è scritto in **Rust `no_std`**,
bootato da **Limine**.

- Bootloader: Pure64 → **Limine** (fornisce memory map e framebuffer pronti).
- Toolchain cross-gcc (ex `x64barebones/Toolchain/`): rimossa.
- Linguaggio: C → **Rust nightly**.

## Step 1 — Toolchain Rust nightly + target

- Cartella `Toolchain/` cross-gcc rimossa (non serve più).
- Installa Rust nightly + componenti necessari (`rust-src` per build-std,
  `llvm-tools-preview`).
- **Target:** usa `x86_64-unknown-none` (target ufficiale dal Rust 1.62 — niente più
  target spec custom `x86_64-myos.json` per i casi semplici).
- Configura cargo con `build-std=core,alloc,compiler_builtins` in `.cargo/config.toml`.

## Step 2 — Build: cargo + piccolo Makefile orchestratore

- **Cargo** compila il kernel Rust.
- **Makefile** sopra cargo per: assemblare i file `.asm` rimasti, linkare tutto con
  un linker script, generare l'ISO con **xorriso** per Limine, lanciare QEMU.
- Non provare a fare tutto da `build.rs` finché non hai un build funzionante.

## Step 3 — "Hello world" Rust al posto del kernel C

- Kernel Rust `#![no_std] #![no_main]`, entry `kernel_main`.
- Output su framebuffer Limine **o** sulla seriale — **consiglio seriale subito**
  (`0x3F8`, già noto dal C): debugging 100x più facile.
- Panic handler che halta. Niente altro.
- Questo è il commit "ora sono in Rust".

## Step 4 — Allocator + heap

- Aggiungi `linked_list_allocator` o `talc` come `#[global_allocator]`.
- Mappa un range virtuale come heap kernel.
- Da qui usi `alloc`: `Vec`, `Box`, `String`, `BTreeMap`. (Senza heap saresti
  costretto a `static` ovunque.)

## Step 5 — IDT, GDT, interrupt

Usa il crate `x86_64` (tipi safe per IDT, GDT, TSS, page tables, registri di controllo —
di fatto standard per OS Rust su x86-64). Porta la conoscenza dal C ma in modo che il
compilatore aiuti. Implementa:

- Eccezioni base (DE, UD, GP, PF, DF su stack **IST** separato).
- Remap IRQ dal PIC (oppure setup APIC se vuoi essere moderno — più lavoro).
- Handler tastiera PS/2 e timer PIT — portati quasi 1:1 dal C.

## Step 6 — Physical frame allocator + paging (Rust-side)

- Parsing della memory map (Limine la fornisce pronta).
- Bitmap allocator per i frame fisici.
- Sopra: API per mappare/smappare pagine usando `x86_64::structures::paging`.
- Qui Rust comincia a pagare: `PhysAddr`/`VirtAddr` distinti, flag PTE come bitflag
  tipati, trait `Mapper`. I bug C tipo "confuso fisico e virtuale" diventano errori
  di compilazione.

## Step 7 — Tasking

Decisione: thread kernel-style vs async → per l'obiettivo Podman vai **thread-style**.
Implementa:

- **TCB** (`Task`: stato registri, stack kernel, stack user, root della page table).
- **Context switch** in asm (resta asm: `global_asm!` o file `.s` separato, ~30 righe).
- **Scheduler** round-robin con `VecDeque<Arc<Task>>`.
- **Cooperative prima, preemptive dopo** (timer IRQ → reschedule).

## Step 8 — User mode + syscall

- GDT con segmenti ring 3, TSS con RSP0.
- Setup MSR per syscall/sysret: `IA32_LSTAR`, `IA32_STAR`, `IA32_FMASK`.
- Tabella syscall in Rust.
- Userland: per ora binari custom; libc vera dopo.

## Step 9 — VFS + un fs reale

- Trait `FileSystem`, `Inode`, `File`.
- Implementa **tmpfs** (banale), poi **FAT** (più semplice di ext2 per partire —
  esiste il crate `fatfs` no_std).
- Poi block layer + driver disco: **virtio-blk** (crate `virtio-drivers`, 10x più
  facile in QEMU), poi AHCI.

## Note

- Lo step "Codice / Installa:" dello Step 1 nel brief originale era vuoto: i comandi
  esatti (rustup, componenti, crate) si fissano nella spec/piano dello Step 1.
- Ordine di dipendenza stretto: ogni step poggia sul precedente. Heap (4) prima di
  strutture dinamiche; interrupt (5) prima di preemption (7); paging (6) prima di
  user mode (8).
