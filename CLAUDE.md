# CLAUDE.md — Regole del progetto MinimalOS

## Progetto

OS hobby x86-64 basato su bootloader Pure64, emulato con QEMU. Obiettivo:
evolverlo in un OS più completo che parte su PC reali x86-64 via boot USB.

Codice sorgente in `x64barebones/` (Bootloader Pure64, Kernel C, Userland).

### Roadmap (sotto-progetti, in ordine di dipendenza)

1. **Gestore memoria** — frame allocator (bitmap, da E820) + paging 4 KiB
   (map/unmap/createAddressSpace) + heap kernel buddy (`kmalloc`/`kfree`).
   *Fondamenta.* Spec: `docs/superpowers/specs/2026-05-27-memory-manager-design.md`.
2. **Multitasking** — scheduler preemptive, processi, syscall (fork/exec/exit/wait),
   timer IRQ0. Dipende da #1.
3. **Driver disco + filesystem** — ATA/AHCI + FS read/write. Dipende da #1.
4. **Portabilità HW reale** — video VESA, tastiera robusta, rilevamento hardware.
   Parzialmente parallelo.
5. **Comandi shell** — sfruttano le nuove capacità (ps, kill, ls, cat, ...).
   Dipende da #1–#4.

Ogni sotto-progetto ha il suo ciclo spec → piano → implementazione.

## Ambiente di build (WSL Ubuntu)

Il build è Linux-only (gcc ELF64, ld, nasm, qemu). Su questa macchina il toolchain
vive nella distro **WSL Ubuntu** (root). Il repo è visibile da WSL a
`/mnt/e/MinimalOS/BasicOperatingSystem`.

- **Tutti** i comandi `make`/`qemu` vanno eseguiti via WSL, es.:
  ```bash
  wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem/x64barebones && make all'
  ```
- **Prima volta:** il `make all` top-level NON builda il Toolchain. Costruire una
  volta il ModulePacker: `cd Toolchain && make all` (produce `mp.bin`). Poi
  `make all` dalla root di `x64barebones`.
- Output immagine: `Image/x64BareBonesImage.qcow2`.
- Test headless con seriale catturato a terminale (vedi `runtest.sh`):
  `qemu-system-x86_64 ... -serial stdio -display none -device isa-debug-exit,iobase=0xf4,iosize=0x04`.

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
