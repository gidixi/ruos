# 442 — init: tool unitctl

**Data:** 2026-06-11

## Cosa
`user/unitctl/` (nuovo tool `.wasm` wasmi): subcomandi `list`, `status`,
`start`, `stop`, `enable`, `disable`, `timers`, `reload`, `cat` (legge il
file sorgente via WASI dalla colonna file dello status). Wired:
`user/Cargo.toml` members + `BIN_TOOLS` nel Makefile (finisce in
`bin.bgz` → `/bin`). Convive col tool `service` legacy.

## Perché
Fase 8 spec init-units: CLI utente dell'init system.

## File toccati
- user/unitctl/Cargo.toml
- user/unitctl/src/main.rs
- user/Cargo.toml
- Makefile
