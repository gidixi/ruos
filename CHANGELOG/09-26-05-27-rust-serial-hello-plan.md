# 09 — Piano implementazione: Rust serial hello (Limine)

**Data:** 2026-05-27

## Cosa

Scritto il piano del primo milestone Rust in
`docs/superpowers/plans/2026-05-27-rust-serial-hello.md`. Tre task:

1. Toolchain Rust nightly in WSL (rustup + rust-src/llvm-tools, curl/xorriso) +
   rimozione `x64barebones/Toolchain/`.
2. Crate `kernel/` no_std (Cargo/toolchain/cargo config, linker.ld higher-half,
   main.rs con richieste Limine + panic halt, serial.rs su COM1) → build a ELF.
3. limine.conf + Makefile (cargo → ISO xorriso/Limine → QEMU) + boot headless che
   verifica la stringa seriale "MinimalOS-rs: hello serial"; pin del nightly.

## Perché

Tradurre la spec del milestone "ora sono in Rust" in passi eseguibili e verificabili.

## File toccati

- docs/superpowers/plans/2026-05-27-rust-serial-hello.md
- CHANGELOG/09-26-05-27-rust-serial-hello-plan.md
