# 356 — netconsole-rx host tool + build-iso prompt

**Data:** 2026-06-08

## Cosa
- Nuovo tool host `tools/netconsole-rx/` (Rust std, zero-dep, cross-platform):
  bind UDP (default `0.0.0.0:6666`), stampa i datagram netconsole su stdout.
  Drop-in per `nc -ul 6666`. Flag `-p/--port`, `--bind`, `--src` (prefisso IP
  sorgente), `-h`. Banner + errori su stderr. Mirror di tutto l'output in
  `netconsole.log` nella cartella dell'eseguibile, troncato a ogni avvio
  (solo la sessione corrente). Testato in loopback (incl. troncamento al
  restart). Cross-compila a `.exe` Windows (`x86_64-pc-windows-gnu`).
- `build-iso.ps1`: aggiunto switch `-Netconsole` + prompt interattivo
  (default **NO**) che attiva la feature `netconsole` nella build.

## Perché
ruos con `--features netconsole` manda i log kernel in UDP broadcast :6666.
Serviva un ricevitore comodo lato host (anche dove manca nc/ncat) e un modo
semplice di abilitare la feature dallo script di build Windows.

## File toccati
- tools/netconsole-rx/Cargo.toml (nuovo)
- tools/netconsole-rx/src/main.rs (nuovo)
- tools/netconsole-rx/README.md (nuovo)
- build-iso.ps1
