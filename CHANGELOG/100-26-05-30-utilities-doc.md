# 100 — Doc riferimento utility userland

**Data:** 2026-05-30

## Cosa

Creato `docs/utilities.md`: riferimento completo delle 45 utility WASM in
`user/*/src/main.rs`. Per ogni tool documenta funzione, comportamento implementato
(flag/argomenti onorati, host fn ruos / WASI usate, internals, limiti hardcoded) e
cosa manca rispetto alla versione reale (GNU coreutils / util-linux / POSIX). Include
sezione architettura comune (host fn pattern, formato record `readdir`, buffer cap,
parsing flag) e pattern/limiti trasversali (dati hardcoded, retry ENOBUFS, niente
regex/pipe/permessi).

## Perché

Mappare lo stato reale della userland: distinguere cosa è funzionante da cosa è
stub/finto/hardcoded, in vista degli step successivi (shell con pipe, modello
permessi/timestamp, regex).

## File toccati

- docs/utilities.md
- CHANGELOG/100-26-05-30-utilities-doc.md
