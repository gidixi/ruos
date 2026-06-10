# 411 — fase unpack_bin (bin.bgz → /bin), rimuove media_bin

**Data:** 2026-06-10

## Cosa
Aggiunta nuova fase di boot `unpack_bin` che decomprime l'archivio `bin.bgz`
(modulo Limine caricato in HHDM) in tmpfs `/bin`, usando `gzip_core::pack`.
La fase si inserisce subito dopo `fs::init()`, prima di storage/usb, eliminando
la dipendenza dal supporto rimovibile (ATAPI/USB-MSC).

Rimossa la vecchia fase `media_bin` e il task asincrono differito `bin_overlay_task`
dell'executor. Il file `kernel/src/boot/phases/media_bin.rs` è stato cancellato
(git rm).

Fallback: se `bin.bgz` manca o è corrotto, `rescue_fallback()` scrive i moduli
`/rescue/*` in `/bin`; se anche quelli mancano, il kernel va in panic.

## Perché
Task 6 del piano gzip-tools: rendere il set `/bin` indipendente dal medium di
boot (USB/ATAPI), leggendolo direttamente dalla RAM HHDM dove Limine ha già
caricato il modulo `bin.bgz`.

## File toccati
- `kernel/src/boot/phases/unpack_bin.rs` (nuovo)
- `kernel/src/boot/phases/media_bin.rs` (rimosso)
- `kernel/src/boot/phases/mod.rs`
- `kernel/src/boot/mod.rs`
- `kernel/src/executor/mod.rs`
