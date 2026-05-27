# CLAUDE.md — Regole del progetto MinimalOS

## Progetto

OS hobby x86-64. **Direzione corrente: riscrittura in Rust `no_std` con bootloader
Limine.** North star a lungo termine: far girare **Podman/container** (implica, col
tempo, user mode, syscall, VFS + fs reale, compat Linux).

### Stato

- **Legacy C (riferimento)** in `x64barebones/` — kernel C su Pure64 + un gestore
  memoria completo (frame allocator bitmap da E820, paging 4 KiB, heap buddy,
  syscall). NON è la base del nuovo OS: serve come **riferimento di conoscenza**
  (logica E820/bitmap/buddy/paging) mentre si riscrive in Rust. Plan/spec del C:
  `docs/superpowers/plans/2026-05-27-memory-manager.md`.
- **Nuovo OS Rust** — vedi roadmap sotto. Sostituisce gradualmente il kernel C;
  bootloader Pure64 → **Limine**; cartella `Toolchain/` (cross-gcc) → **eliminata**.

### Roadmap Rust (dettaglio completo: `docs/superpowers/roadmap-rust-os.md`)

1. **Toolchain Rust nightly + target.** Target `x86_64-unknown-none` (ufficiale dal
   1.62, niente target custom). `build-std=core,alloc,compiler_builtins` in
   `.cargo/config.toml`. Elimina `Toolchain/`.
2. **Build: cargo + Makefile orchestratore.** Cargo compila il kernel Rust; Makefile
   assembla gli `.asm` rimasti, linka con linker script, genera ISO con `xorriso`
   per Limine, lancia QEMU. (Non spostare tutto in `build.rs` finché il build non gira.)
3. **Hello world Rust** al posto del kernel C. `#![no_std] #![no_main]`, entry
   `kernel_main`, output su **seriale `0x3F8`** (debug facile), panic handler che halta.
   Commit "ora sono in Rust".
4. **Allocator + heap.** `linked_list_allocator` o `talc` come `#[global_allocator]`;
   mappa un range virtuale come heap kernel → abilita `alloc` (Vec/Box/String/BTreeMap).
5. **IDT, GDT, interrupt** col crate `x86_64`. Eccezioni base (DE/UD/GP/PF, DF su IST),
   remap PIC (o APIC), handler tastiera PS/2 + timer PIT (portati ~1:1 dal C).
6. **Frame allocator fisico + paging Rust.** Memory map da Limine; bitmap allocator;
   API map/unmap con `x86_64::structures::paging` (PhysAddr/VirtAddr tipati, PTE bitflag).
7. **Tasking** thread-style (per goal Podman). TCB (regs/stack kernel+user/page-table
   root), context switch in asm (`global_asm!`/`.s`), scheduler round-robin
   `VecDeque<Arc<Task>>`. Cooperative → poi preemptive (timer IRQ → reschedule).
8. **User mode + syscall.** GDT ring 3, TSS con RSP0, MSR `IA32_LSTAR/STAR/FMASK`
   (syscall/sysret), tabella syscall in Rust. Userland binari custom, libc dopo.
9. **VFS + fs reale.** Trait `FileSystem`/`Inode`/`File`. tmpfs → FAT (`fatfs` no_std)
   → block layer + driver disco (virtio-blk via `virtio-drivers` in QEMU, poi AHCI).

Ogni step ha il suo ciclo spec → piano → implementazione.

## Ambiente di build

**Host build = WSL Ubuntu** (root). Repo visibile a `/mnt/e/MinimalOS/BasicOperatingSystem`.
Comandi build/run vanno eseguiti via WSL, es.:
```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && <cmd>'
```

- **Nuovo OS Rust (target):** rustup nightly + componenti `rust-src`, `llvm-tools`;
  `cargo build` con `build-std`; ISO Limine via `xorriso`; run con `qemu-system-x86_64`.
  Da installare allo Step 1 (vedi roadmap doc).
- **Legacy C (solo per il riferimento):** gcc ELF64 + nasm + qemu già installati in WSL;
  build con `cd x64barebones && cd Toolchain && make all` (una volta) poi `make all`;
  test seriale headless via `x64barebones/runtest.sh`. Questo toolchain resta solo
  finché serve consultare/eseguire il C; `Toolchain/` verrà eliminata allo Step 1.

## Regole di lavoro (OBBLIGATORIE)

### Changelog — una entry per ogni modifica

Per **ogni modifica** al repository (codice, spec, config, doc) creo un file in
`CHANGELOG/` con nome:

```
NN-yy-mm-dd-slug.md
```

- `NN` = contatore progressivo a 2 cifre, parte da `00`, incrementa di 1 ad ogni
  nuova entry (mai riusato).
- `yy-mm-dd` = data della modifica.
- `slug` = descrizione breve in kebab-case.

Contenuto di ogni entry:

```markdown
# NN — <titolo breve>

**Data:** yyyy-mm-dd

## Cosa
<cosa è cambiato>

## Perché
<motivo>

## File toccati
- path/file1
- path/file2
```

Prima di creare una entry, controllo il numero più alto già presente in
`CHANGELOG/` e uso il successivo.

### Git

- **Non fare commit/push se non richiesto esplicitamente** dall'utente.
- Se sul branch di default (`master`/`main`), creare prima un branch.

### Spec e design

- Le spec di design vanno in `docs/superpowers/specs/YYYY-MM-DD-<topic>-design.md`.

### Stile

- Codice nuovo segue lo stile del codice circostante (naming, commenti, idiomi).
