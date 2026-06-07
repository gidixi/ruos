# 301 — build-iso.ps1 portabile su PC/ambienti diversi

**Data:** 2026-06-06

## Cosa

Resa robusta `build-iso.ps1` per funzionare su macchine e ambienti diversi,
provvedendo automaticamente a ciò che manca:

- **Auto-rilevamento distro WSL.** `-Distro` ora è opzionale (default `""`): se
  omesso (o se nomina una distro non installata) lo script ne seleziona una
  valida preferendo Ubuntu, poi Debian, escludendo le distro-helper
  `docker-desktop*` / `rancher*`. Output WSL letto forzando UTF-16LE e ripulito
  dai byte non stampabili. Se WSL o nessuna distro è presente, messaggio
  d'errore con il comando per installarla.
- **Auto-install dipendenze di sistema (step 1, nuovo).** Verifica e installa via
  `apt-get` solo i pacchetti mancanti: `build-essential` (gcc/make), `xorriso`,
  `mtools`, `qemu-system-x86`, `python3`, `curl`, `git`. Usa `sudo` se non root,
  `DEBIAN_FRONTEND=noninteractive`. Guard per distro non-apt. Flag `-SkipDeps`
  per saltare il check.
- **Auto-install rustup (step 2).** Se `rustup` è assente lo installa
  non-interattivo (`--default-toolchain none --no-modify-path`); il nightly
  pinnato + componenti arrivano da `rust-toolchain.toml` al primo uso. Aggiunge i
  target `wasm32-wasip1` e `wasm32-unknown-unknown`.
- **Fix lista `.cwasm` pre-build.** Aggiunto `egui_demo.cwasm` ai guest
  pre-buildati prima del kernel: il kernel lo fa `include_bytes!` (con reactor /
  reactor_close / probe) e su un working tree pulito la build falliva con
  "No such file or directory" perché lo step lo ometteva.

## Perché

Lo script presupponeva un ambiente già provvisto e una distro hardcoded
(`Ubuntu-22.04`) non presente su tutte le macchine; inoltre ometteva
`egui_demo.cwasm` dalla pre-build, facendo fallire `make iso` da clean tree.
Verificato end-to-end (distro auto-rilevata `Ubuntu`, build OK, `build/os.iso`
prodotta).

## File toccati

- build-iso.ps1
- CHANGELOG/301-26-06-06-build-iso-ps1-portable.md
