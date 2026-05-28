# 22 — Rinomina prefisso seriale e nome progetto a `ruos`

**Data:** 2026-05-28

## Cosa

- `kernel/src/main.rs`: tutte le 4 log line cambiano prefisso da
  `MinimalOS-rs:` a **`ruos:`** (hello, unsupported base revision, heap fail,
  heap ok, alloc box).
- `Makefile`: variabile `HELLO` aggiornata alla nuova stringa
  `ruos: alloc box=0xCAFEBABE vec=[0, 1, 2, 3, 4]`.
- `limine.conf`: entry del boot menu da `/MinimalOS-rs` a `/ruos`.
- `README.md`: tre esempi di output seriale aggiornati al nuovo prefisso.
- `CLAUDE.md`: titolo `Regole del progetto MinimalOS` → `Regole del progetto ruos`.
- Changelog/spec/plan storici NON toccati: documentano l'output al momento
  dell'implementazione.

## Perché

Nome ufficiale del SO = **ruos** (anche nome del repo GitHub). Stringhe seriali
e configurazioni boot allineate.

## File toccati

- kernel/src/main.rs
- Makefile
- limine.conf
- README.md
- CLAUDE.md
- CHANGELOG/22-26-05-28-rename-to-ruos.md
