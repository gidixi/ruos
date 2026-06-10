# 403 — gzip/gunzip/zcat: CLI condivisa + tre bin + wiring build

**Data:** 2026-06-10

## Cosa

Completati i tool di compressione sopra `gzip-core` (compress/decompress già
fatti):

- `gzip-core::run_cli(default_decompress, default_stdout)` — entrypoint CLI
  condiviso: parsing flag (`-c -k -d -1..-9 -h`, combinabili), dispatch
  file/stdin, semantica Unix (`gzip f`→`f.gz` + unlink; `gunzip f.gz`→`f`;
  niente overwrite silenzioso; più file in sequenza, exit 1 se almeno uno
  fallisce; senza file stdin→stdout).
- Tre bin thin `wasm32-wasip1`: `gzip` (`run_cli(false,false)`),
  `gunzip` (`run_cli(true,false)`), `zcat` (`run_cli(true,true)`). Ognuno
  `ruos_rt::init()` + chiamata a run_cli.
- Wiring: membri workspace `gzip/gunzip/zcat` in `user/Cargo.toml`;
  `gzip gunzip zcat` in `BIN_TOOLS` del Makefile.

Verifica: `cargo test -p gzip-core` 18/18 ok; build release dei tre bin su
`wasm32-wasip1` ok.

## Perché

Chiudere lo step gzip (spec 401, plan 402): la lib era pronta ma mancavano i
front-end CLI e l'inserimento nella build/ISO.

## File toccati
- user/gzip-core/src/cli.rs (nuovo)
- user/gzip-core/src/lib.rs
- user/gzip/Cargo.toml, user/gzip/src/main.rs (nuovi)
- user/gunzip/Cargo.toml, user/gunzip/src/main.rs (nuovi)
- user/zcat/Cargo.toml, user/zcat/src/main.rs (nuovi)
- user/Cargo.toml
- Makefile
