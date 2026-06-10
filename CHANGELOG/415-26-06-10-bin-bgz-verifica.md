# 415 — Verifica live-CD bin.bgz (gate QEMU + hardware reale)

**Data:** 2026-06-10

## Cosa
Verifica end-to-end della feature `/bin` via `bin.bgz`:
- `make run-test` (CD ATAPI) → `TEST_PASS`; log `unpack_bin unpacked 66 bins from
  bin.bgz (0 failed)` in ~1.9s; heap 384 MiB (`size=402653184`); `/bin/ls.wasm` +
  `/bin/wc.wasm` eseguiti da tmpfs.
- `make run-test-usb` (boot da chiavetta USB, bin.bgz caricato da Limine via
  firmware) → `TEST_PASS_USB`.
- **Hardware reale**: boot OK, `/bin` popolato, USB scollegabile. Il panic OOM
  precedente (membro `.cwasm` grosso) è risolto dal pre-dimensionamento ISIZE
  (changelog 413).

## Perché
Chiudere il piano bin-pack-livecd con evidenza: nessun OOM, picco RAM entro lo
heap da 384 MiB, dipendenza runtime da USB-MSC/ATAPI eliminata.

## File toccati
- CHANGELOG/415-26-06-10-bin-bgz-verifica.md
