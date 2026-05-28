# CLAUDE.md — Regole del progetto ruos

## Progetto

OS hobby x86-64. **Direzione corrente: riscrittura in Rust `no_std` con bootloader
Limine.** North star a lungo termine: far girare **Podman/container** (implica, col
tempo, user mode, syscall, VFS + fs reale, compat Linux).

### Stato

- **Codice attivo**: kernel Rust `no_std` in `kernel/` + Makefile root + `limine.conf`.
  Bota da Limine in QEMU, heap funzionante (talc + Limine memmap/HHDM). Vedi
  roadmap sotto.
- **Legacy C (rimosso)** — il vecchio kernel C su Pure64 + gestore memoria
  (E820/bitmap/buddy/paging) viveva in `x64barebones/`. Rimosso dal working tree;
  resta come **riferimento storico in git history** fino al commit `c1d2a81`
  (plan/spec a `docs/superpowers/plans/2026-05-27-memory-manager.md` e
  `docs/superpowers/specs/2026-05-27-memory-manager-design.md`).

### Roadmap Rust (dettaglio completo: `docs/superpowers/roadmap-rust-os.md`)

1. **Toolchain Rust nightly + target.** Target `x86_64-unknown-none` (ufficiale dal
   1.62, niente target custom). `build-std=core,alloc,compiler_builtins` in
   `.cargo/config.toml`. ✅ FATTO.
2. **Build: cargo + Makefile orchestratore.** Cargo compila il kernel Rust; Makefile
   assembla gli `.asm` rimasti, linka con linker script, genera ISO con `xorriso`
   per Limine, lancia QEMU. ✅ FATTO.
3. **Hello world Rust** `no_std`/`no_main`, output seriale `0x3F8`, panic handler
   che halta. ✅ FATTO.
4. **Allocator + heap.** `talc` come `#[global_allocator]`, heap 4 MiB da Limine
   memory map + HHDM, `alloc` (Vec/Box/String/BTreeMap) abilitato. ✅ FATTO.
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

- **Toolchain installato in WSL:** rustup nightly (`nightly-2026-05-26`) +
  componenti `rust-src` e `llvm-tools-preview`; `xorriso`, `qemu-system-x86_64`,
  `gcc`/`make` (per buildare il tool host `limine`).
- **Build:** `make iso` dalla root del repo (clona Limine v11.4.1-binary la prima
  volta in `third_party/limine/`, builda il kernel Rust, assembla ISO).
- **Test:** `make run-test` → boot headless con seriale a stdio, asserisce la
  stringa di successo (vedi `Makefile` variabile `HELLO`).
- **Run interattivo:** `make run` (QEMU con display).

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
