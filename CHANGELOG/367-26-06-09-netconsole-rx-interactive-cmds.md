# 367 — netconsole-rx: comandi interattivi da tastiera

**Data:** 2026-06-09

## Cosa
Aggiunti comandi runtime a `tools/netconsole-rx` (digita + Invio su stdin):
- `c`/`clear`/`cls` → pulisce lo schermo del terminale (ANSI `\x1b[2J\x1b[H`)
- `l`/`clog`/`clearlog` → svuota il log file (`netconsole.log` troncato)
- `h`/`help`/`?` → lista comandi
- `q`/`quit`/`exit` → esce

Implementati con un thread stdin separato; il log file è ora condiviso col loop
di ricezione via `Arc<Mutex<File>>`. stdin a righe (no raw mode) → resta
zero-dipendenze, cross-platform. Comandi inerti se stdin è in pipe/redirect (la
ricezione UDP continua comunque). README + help (`-h`) aggiornati.

## Perché
Durante debug bare-metal il log scrolla in fretta: poter pulire schermo e
troncare il log al volo senza riavviare il ricevitore.

## File toccati
- tools/netconsole-rx/src/main.rs
- tools/netconsole-rx/README.md
