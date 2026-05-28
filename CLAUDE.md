# CLAUDE.md — Regole del progetto ruos

## Progetto

OS hobby x86-64 in Rust `no_std` con bootloader **Limine**. **North star (pivot
2026-05-28): eseguire app `.wasm` (WASI), avere GUI (rlvgl) e accesso remoto via
SSH.** Tutto userspace = moduli WebAssembly; il runtime WASM è il sandbox.

### Cosa NON faremo (drop espliciti dal pivot)

- **Niente Linux ABI / ELF userland.** App = `.wasm` compilate `wasm32-wasi`,
  non binari ELF Linux. Niente Podman, niente compat libc Linux.
- **Niente user-mode CPU ring 3** (no SYSCALL/SYSRET MSR, no GDT ring 3 attivo).
  Sandbox = WASM, non page tables + privilegi CPU. Tutto gira in ring 0 con
  isolamento garantito dal runtime.
- **Niente preemptive thread scheduler.** Concurrency = **async cooperative**
  (executor no_std, timer IRQ → wake), single-CPU. SMP dopo, se serve.

### Stato

- **Codice attivo**: kernel Rust `no_std` in `kernel/` + Makefile root + `limine.conf`.
  Boot Limine → seriale → heap → smoke alloc → GDT/TSS → IDT → ACPI → LAPIC/IOAPIC
  → timer 100 Hz → tastiera PS/2 → halt. Verificato in QEMU, VirtualBox, ISO USB.
- **Legacy C (rimosso)** — il vecchio kernel C su Pure64 + gestore memoria
  (E820/bitmap/buddy/paging) viveva in `x64barebones/`. Rimosso dal working tree;
  resta come **riferimento storico in git history** fino al commit `c1d2a81`
  (plan/spec a `docs/superpowers/plans/2026-05-27-memory-manager.md` e
  `docs/superpowers/specs/2026-05-27-memory-manager-design.md`).

### Roadmap (dettaglio completo: `docs/superpowers/roadmap-rust-os.md`)

**Fondamenta (5 step, tutti fatti):**

1. **Toolchain Rust nightly + target** `x86_64-unknown-none` + `build-std`. ✅ FATTO.
2. **Build cargo + Makefile orchestratore + Limine ISO via xorriso.** ✅ FATTO.
3. **Hello world `no_std`/`no_main` + seriale COM1 + panic halt.** ✅ FATTO.
4. **Heap + global allocator (`talc`)** su Limine memmap+HHDM, 4 MiB,
   `alloc` (Vec/Box/String/BTreeMap) abilitato. ✅ FATTO.
5. **IDT/GDT + APIC + timer 100 Hz + tastiera PS/2 IRQ1.** ✅ FATTO.

**Base mancante per WASM userland (in ordine di dipendenza):**

6. **Frame allocator fisico + paging API completata.** Bitmap da E820,
   `map/unmap_page` generico, gestione reserve regions (heap, kernel, MMIO).
   NO per-process page tables, NO ring 3.
7. **VFS minimale + tmpfs in-RAM.** Trait `FileSystem`/`Inode`/`File`, popolato
   a init (es. `/init.wasm`, `/dev/console`). FAT/AHCI dopo, se servirà.
8. **Framebuffer console.** Limine `FramebufferRequest` + font bitmap +
   scrolling + cursor. Trait `Console` (impl seriale + framebuffer).
9. **Async executor `no_std`** (es. `embassy-executor`). Wake source = timer IRQ
   tick. Sostituisce lo scheduler preemptive droppato.
10. **WASM runtime + WASI Preview 1.** `wasmi` (Rust puro, no_std) preferito,
    altrimenti WAMR via FFI. Host functions: `args_get`, `environ_get`,
    `clock_time_get`, `random_get`, `fd_read/write/seek`, `path_*`, `proc_exit`.
11. **Shell locale.** Line editing (←/→/⌫), PATH lookup via VFS, exec `.wasm` via
    runtime, builtin minimali (`cd`, `pwd`, `ls`, `exit`).
12. **PTY.** Pseudo-terminal master/slave, line discipline. La shell gira sopra
    PTY (locale o SSH).
13. **Mouse PS/2 + rlvgl + host functions custom.** Driver mouse PS/2 (IRQ12),
    crate `rlvgl`, host fn `ruos_draw_*`/`ruos_input_event` per app WASM grafiche.
14. **Networking.** Driver `virtio-net` (QEMU/VBox prima), stack TCP `smoltcp`,
    **CSPRNG seedato da RDRAND** (critico per crypto SSH).
15. **SSH server.** `sunset` (no_std/no_alloc, naturale) o `russh` (async+alloc).
    Pubkey hardcoded inizialmente. Exec non-interattivo prima, sessione
    interattiva su PTY dopo.

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
