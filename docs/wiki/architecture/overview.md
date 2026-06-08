# Architettura — panoramica

> **Stato:** stub
> **Aggiornato:** 2026-06-08
> **Fonti:** `kernel/src/boot/mod.rs`, `kernel/src/main.rs`

ruOS è un OS x86-64 in Rust `no_std`, bootato da **Limine**, in cui **tutto lo
userland è WebAssembly**: le app sono moduli `.wasm`/`.cwasm` e il runtime WASM è
la sandbox (niente ring 3, niente ELF Linux, niente thread preemptivi —
concorrenza async cooperativa).

## Boot a fasi

Il boot procede per fasi (`kernel/src/boot/mod.rs`):

```
arch → mem → interrupts (+SMP) → pci → devices (framebuffer + PS/2)
     → fs (VFS/tmpfs) → storage (AHCI/FAT32 /mnt) → usb (xHCI: HID kbd+mouse, hub)
     → userland (RNG, net, SSH, executor async)
```

A regime: shell su PTY, **desktop egui** via [compositor kernel-side](../components/compositor.md),
SSH.

## Due runtime WASM

- **wasmi** — interprete `no_std`, esegue i tool `.wasm` (wasm32-wasip1).
- **Wasmtime AOT** `no_std` — esegue i `.cwasm` precompilati (GUI/compositor +
  Component Model) a velocità quasi-nativa, memoria W^X.

## Da approfondire

Questa pagina è uno stub. Componenti già documentati:
[Compositor](../components/compositor.md). Il resto è elencato nell'[indice](../README.md).
