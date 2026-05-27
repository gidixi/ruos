# 07 — Pivot direzione: riscrittura in Rust (no_std) + Limine

**Data:** 2026-05-27

## Cosa

- Registrata la nuova direzione del progetto: riscrittura del kernel in **Rust
  `no_std`** con bootloader **Limine**, north star = far girare Podman/container.
- Il kernel C su Pure64 (`x64barebones/`, incluso il gestore memoria già fatto)
  diventa **riferimento di conoscenza**, non più la base.
- Nuova roadmap a 9 step in `docs/superpowers/roadmap-rust-os.md` (toolchain Rust →
  build cargo+Makefile → hello world → heap → IDT/GDT → frame+paging → tasking →
  user mode/syscall → VFS+fs).
- CLAUDE.md aggiornato: sezioni Progetto/Stato, Roadmap Rust, Ambiente di build
  (toolchain Rust target vs toolchain C legacy).

## Perché

L'utente ha ridefinito l'obiettivo e lo stack. Rust + crate `x86_64` rendono molti
bug di memoria errori di compilazione; Limine evita il lavoro di basso livello di
Pure64. Il C resta utile come riferimento ma non come fondamenta.

## File toccati

- CLAUDE.md
- docs/superpowers/roadmap-rust-os.md
- CHANGELOG/07-26-05-27-pivot-rust-limine.md
