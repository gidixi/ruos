# 350 — Cosmetica boot: banner allineato + rimossi demo /root

**Data:** 2026-06-08

## Cosa
- **Banner allineato**: le righe dinamiche del box usavano `{:^39}` su
  `format_args!`, ma `Display for Arguments` ignora width/fill → niente padding →
  bordi `|` disallineati. Ora ogni riga dinamica è renderizzata in un buffer su
  stack (`LineBuf`) e centrata come `&str`.
- **Rimossi i demo `/root/server.wasm` e `/root/client.wasm`**: non più caricati
  come moduli Limine (limine.conf + limine-ssd.conf), non più spediti sull'ISO
  (Makefile), tolti dalla lista bootstrap dell'installer (disk.rs). Sparisce il
  `WARN mod install fail /root/*.wasm: NotFound` al boot. (Gli arm socket-demo in
  wasm/mod.rs restano come dead code innocuo.)

## Perché
Pulizia estetica dei log di boot prima di chiudere la feature live-CD.

## File toccati
- kernel/src/boot/banner.rs
- limine.conf, limine-ssd.conf
- Makefile
- kernel/src/disk.rs
- CHANGELOG/350-26-06-08-boot-cosmetics.md
