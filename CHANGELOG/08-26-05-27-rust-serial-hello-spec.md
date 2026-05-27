# 08 — Spec design: Rust serial hello (Limine)

**Data:** 2026-05-27

## Cosa

Scritta la spec del primo milestone Rust (Step 1+2+3 della roadmap): kernel `no_std`
che bota da Limine (BIOS, ISO) in QEMU e stampa su seriale COM1, panic→halt.
`docs/superpowers/specs/2026-05-27-rust-serial-hello-design.md`. Layout `kernel/`
crate, toolchain nightly + build-std target `x86_64-unknown-none`, crate `limine` +
`uart_16550`, build cargo+Makefile+xorriso, test headless seriale.

## Perché

Primo deliverable eseguibile della riscrittura Rust ("ora sono in Rust"). Bundla
Step 1-3 perché toolchain/build da soli non sono testabili.

## File toccati

- docs/superpowers/specs/2026-05-27-rust-serial-hello-design.md
- CHANGELOG/08-26-05-27-rust-serial-hello-spec.md
