# 11 — Crate kernel Rust no_std (skeleton + seriale)

**Data:** 2026-05-27

## Cosa
- Creato il crate kernel/ (Cargo.toml, rust-toolchain.toml, .cargo/config.toml,
  linker.ld higher-half con ENTRY(kmain), src/main.rs no_std/no_main con richieste
  Limine + panic handler che halta, src/serial.rs su COM1 via uart_16550).
- build-std (core/alloc/compiler_builtins) su target x86_64-unknown-none.
- .gitignore aggiornato per build/ e target/.

## Perché
Step 2-3 della roadmap: kernel Rust che compila a un ELF higher-half pronto per Limine.

## Note versioni / adattamenti
- limine "0.4" era yanked su crates.io: usata l'ultima 0.x pubblicata, **limine = "0.6.3"**.
- uart_16550 risolto a **0.3.2**.
- Adattamento API per limine 0.6.3: `RequestsStartMarker`/`RequestsEndMarker` sono in
  `limine::` (root), non in `limine::request`. Import in main.rs cambiato di conseguenza.
  `BaseRevision` resta in `limine::`. Nessun'altra modifica al pattern barebones.
- ELF verificato: Type EXEC, Entry point 0xffffffff80001020 (higher-half).

## File toccati
- kernel/Cargo.toml, kernel/rust-toolchain.toml, kernel/.cargo/config.toml
- kernel/linker.ld, kernel/src/main.rs, kernel/src/serial.rs
- .gitignore
- CHANGELOG/11-26-05-27-kernel-crate.md
