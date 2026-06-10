# 412 — limine.conf: bin.bgz (/archive/) + rescue (/rescue/)

**Data:** 2026-06-10

## Cosa
Rimossi i ~20 moduli `/bin/*.wasm` individuali da `limine.conf` e sostituiti
con un singolo archivio compresso `bin.bgz` (cmdline `/archive/bin.bgz`) più
un set rescue minimale di 6 tool (cmdline `/rescue/*.wasm`).

- Aggiunto: `module_path: boot():/bin.bgz` + `module_cmdline: /archive/bin.bgz`
- Aggiunto: 6 moduli rescue (shell, ls, cat, echo, dmesg, lspci) sotto
  `/rescue/` sia per path che per cmdline.
- Rimossi: tutti i vecchi blocchi `/bin/shell.wasm` … `/bin/which.wasm`
  (Live-CD fallback set + rescue tool set, ~57 righe).

## Perché
Task 7 del piano `feat/gzip-tools`: il nuovo modello di boot usa la fase
`unpack_bin` per decomprimere `bin.bgz` (gzip, singolo file) in tmpfs `/bin`
dopo l'handoff Limine. Il prefisso `/archive/` segnala a `modules.rs` di
saltare il montaggio diretto; il prefisso `/rescue/` identifica il set di
fallback scritto in `/bin` solo se l'unpack fallisce.

## File toccati
- limine.conf
