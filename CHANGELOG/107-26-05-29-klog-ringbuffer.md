# 107 — Kernel log ring buffer

**Data:** 2026-05-29

## Cosa
Nuovo modulo `kernel/src/klog.rs` con ring buffer 32 KiB. `kprintln!`
(in `kprint.rs`) e `boot::log::emit` (binfo/bwarn/berr) ora scrivono
sia alla console che al ring, riformattando in un `klog::Scratch`
(256 byte stack-allocato — messaggi lunghi vengono troncati silenti,
acceptable per dmesg).

`klog::read(out: &mut [u8]) -> usize` esporta i bytes oldest-to-newest
per la successiva host fn `ruos_dmesg`.

## Perché
Userspace `dmesg` ha bisogno di accedere ai messaggi kernel. Senza un
buffer dedicato l'unico storico era il TTY scrollback (volatile).

## File toccati
- kernel/src/klog.rs (nuovo)
- kernel/src/kprint.rs
- kernel/src/boot/log.rs
- kernel/src/main.rs
