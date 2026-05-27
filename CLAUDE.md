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
