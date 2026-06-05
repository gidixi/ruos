# CLAUDE.md — Regole del progetto ruos

## Progetto

OS WASM-first x86-64 in Rust `no_std` con bootloader **Limine**. **North star (pivot
2026-05-28): eseguire app `.wasm` (WASI), avere GUI (**egui**, non più rlvgl) e
accesso remoto via SSH.** Tutto userspace = moduli WebAssembly; il runtime WASM
è il sandbox.

### Cosa NON faremo (drop espliciti dal pivot)

- **Niente Linux ABI / ELF userland.** App = `.wasm` compilate `wasm32-wasi`,
  non binari ELF Linux. Niente Podman, niente compat libc Linux.
- **Niente user-mode CPU ring 3** (no SYSCALL/SYSRET MSR, no GDT ring 3 attivo).
  Sandbox = WASM, non page tables + privilegi CPU. Tutto gira in ring 0 con
  isolamento garantito dal runtime.
- **Niente preemptive thread scheduler.** Concurrency = **async cooperative**
  (executor no_std, timer IRQ → wake), single-CPU. SMP dopo, se serve.

### Stato

- **Codice attivo**: kernel Rust `no_std` in `kernel/` + Makefile root + `limine.conf`
  + submodule `ruos-desktop/` (UI egui). Boot a fasi (`boot/mod.rs`): arch → mem →
  interrupts (+SMP) → pci → devices (framebuffer + PS/2) → fs (VFS/tmpfs) → storage
  (AHCI/FAT32 `/mnt`) → usb (xHCI: tastiera **e mouse** HID + hub + hot-plug) →
  userland (RNG, net, SSH, executor async). A regime: shell su PTY, **desktop egui**
  (Wasmtime AOT) con **compositor kernel-side** multi-finestra, SSH. Roadmap step
  1-19 (sotto) tutti ✅. Verificato in QEMU, VirtualBox, e su **hardware reale**
  (USB input, GUI, installer SSD).
- **Due runtime WASM**: `wasmi` (interprete, esegue i tool `.wasm` wasm32-wasip1) +
  **Wasmtime AOT** no_std (esegue i `.cwasm` precompilati: GUI/compositor + Component
  Model). Il router `.cwasm` della shell sceglie Wasmtime.
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
4. **Heap + global allocator (`talc`)** su Limine memmap+HHDM, 128 MiB,
   `alloc` (Vec/Box/String/BTreeMap) abilitato. ✅ FATTO.
5. **IDT/GDT + APIC + timer 100 Hz + tastiera PS/2 IRQ1.** ✅ FATTO.

**Base WASM userland (tutti ✅ FATTO):**

6. **Frame allocator fisico + paging API.** Bitmap da Limine memmap,
   `map/unmap_page` generico, reserve regions (heap, kernel, MMIO), frame DMA.
   NO per-process page tables, NO ring 3. ✅
7. **VFS + tmpfs in-RAM** + FAT32 (`/mnt`) + device file (`/dev/{console,null,zero,pts/N}`).
   `fd_readdir` esposto. ✅
8. **Framebuffer console.** Font bitmap + AA blend + scrolling + cursor + parser
   ANSI `vte`. ✅
9. **Async executor `no_std`** (`embassy-executor`). Wake = timer IRQ tick. ✅
10. **WASM runtime + WASI Preview 1** (`wasmi`, no_std) + fiber per call bloccanti
    + fuel metering + ResourceLimiter + un solo accessor memoria guest auditato. ✅
11. **Shell locale.** Line editing, tab-completion, PATH (`/bin` poi `/mnt/bin`),
    pipeline `a | b`, builtin, exec `.wasm`/`.cwasm`. ✅
12. **PTY.** Coppie master/slave + line discipline (cooked, Ctrl-C, echo). ✅
13. **Mouse (PS/2 IRQ12 **+ USB HID**) + GUI egui + host fn grafiche.** Driver mouse
    PS/2 e USB → coda `MouseEvent` comune; servizio framebuffer `gfx` (ABI
    `ruos_gfx`: blit RGBA8888 + eventi tastiera/mouse). GUI = **egui** (non rlvgl),
    sviluppata nel submodule `ruos-desktop` e rasterizzata con `tiny-skia`. ✅
14. **Networking.** `virtio-net` + Intel `e1000`, stack `smoltcp`, DHCP, **CSPRNG
    ChaCha20 seedato da RDRAND**. ✅
15. **SSH server.** `sunset` (no_std), host key ed25519, auth password (PBKDF2) +
    pubkey, shell PTY interattiva + exec, gira anche diskless. ✅

**GUI / desktop (oltre i 15, tutti ✅ FATTO):**

16. **Wasmtime AOT no_std** (runtime-only, no JIT) per eseguire `.cwasm`
    precompilati a velocità quasi-nativa + memoria eseguibile W^X. ✅
17. **Desktop egui end-to-end** (`gui.cwasm`) sul servizio `gfx`, con cursore
    software, rendering dirty-rect, clock monotonico. ✅
18. **Bridge kernel↔WASM via WIT / Component Model** (wasmtime component-model
    no_std) per ABI tipizzate. ✅
19. **Compositor / window manager kernel-side** — ogni finestra è un'app WASM
    separata; input routing + click-to-focus, decorazioni + drag/raise/close,
    compositing SMP-parallelo a bande, launcher + lifecycle spawn/despawn. ✅

Ogni step ha il suo ciclo spec → piano → implementazione.

## Ambiente di build

**Host build = WSL** (distro **`Ubuntu`**, utente root). Repo visibile a
`/mnt/e/MinimalOS/BasicOperatingSystem`. Comandi build/run vanno eseguiti via WSL, es.:
```bash
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && <cmd>'
```

- **Toolchain installato in WSL:** rustup nightly (`nightly-2026-05-26`) +
  componenti `rust-src` e `llvm-tools-preview` + target `wasm32-wasip1` (tool/UI)
  e `wasm32-unknown-unknown` (finestre compositor); `xorriso`, `qemu-system-x86_64`,
  `gcc`/`make` (per buildare il tool host `limine`).
- **Submodule:** `git submodule update --init --recursive` (serve `ruos-desktop`
  per buildare `gui.cwasm`). Su `/mnt/e` può servire
  `git config --global --add safe.directory '*'` (dubious-ownership).
- **Build:** `make iso` dalla root del repo (clona Limine v11.4.1-binary la prima
  volta, builda kernel + tool WASM + desktop egui + `.cwasm` AOT, assembla ISO).
- **Test:** `make run-test` → boot headless con seriale a stdio, asserisce la
  stringa di successo (vedi `Makefile` variabile `HELLO`). Self-test in-boot con
  `make iso CARGO_FEATURES=boot-checks`.
- **Run interattivo:** `make run` (QEMU con display).
- **Git remote:** push/pull/fetch solo da WSL (le credenziali stanno lì); HTTPS
  → richiede auth interattiva, non gira in background non-interattivo.

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
