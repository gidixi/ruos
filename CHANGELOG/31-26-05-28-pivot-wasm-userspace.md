# 31 — Pivot architetturale: userland = WASM (drop Linux ABI)

**Data:** 2026-05-28

## Cosa

Cambio di paradigma sul "cosa è una app" e sul modello di esecuzione:

- **North star aggiornato**: eseguire app `.wasm` (compilate `wasm32-wasi`),
  con GUI via `rlvgl` (host functions custom) e accesso remoto via SSH.
- **Drop espliciti**:
  - Linux ABI / ELF userland / `fork`/`exec`/Linux syscalls / libc Linux /
    Podman containers.
  - User-mode CPU ring 3 (SYSCALL/SYSRET MSR, GDT ring 3 attiva, TSS RSP0
    cross-ring). Sandbox = WASM, non page tables + privilegi CPU.
  - Preemptive thread scheduler. Concurrency = async cooperative
    (`embassy-executor`-style), timer IRQ → wake.
- **Roadmap riscritta**: 5 step già fatti (1-5 fondamenta) + 10 nuovi (6-15)
  in ordine di dipendenza:
  6. Frame allocator + paging API (no per-process page tables)
  7. VFS minimale + tmpfs in-RAM
  8. Framebuffer console (Limine FB + font + scrolling, trait Console)
  9. Async executor no_std (`embassy-executor`)
  10. WASM runtime + WASI Preview 1 (`wasmi` Rust puro preferito)
  11. Shell locale (line editor + exec `.wasm`)
  12. PTY (pseudo-terminal master/slave)
  13. Mouse PS/2 + `rlvgl` + host functions grafiche custom
  14. Networking (`virtio-net` + `smoltcp` + CSPRNG ChaCha20 seedato RDRAND)
  15. SSH server (`sunset` preferito; exec non-interattivo → sessione PTY)

## Perché

Discussione architetturale: WASM come superficie userland è drasticamente più
piccola di Linux-ABI, sandbox gratis, sintonia con Rust no_std. Le app
moderne (Rust/C/Go/Zig) compilano a `wasm32-wasi`. La complessità del kernel
non-WASM (ring 3, syscall ABI, fork/exec, ELF loader) sparisce.

## File toccati

- CLAUDE.md (riscritte sezioni Progetto, Cosa NON faremo, Roadmap)
- docs/superpowers/roadmap-rust-os.md (riscritto completamente, 15 step + diagramma dipendenze + decisioni tecniche fissate)
- CHANGELOG/31-26-05-28-pivot-wasm-userspace.md
