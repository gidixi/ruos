# 87 — Boot infra: banner + logger + error + build.rs (T1)

**Data:** 2026-05-29

## Cosa

- `kernel/build.rs`: emette `RUOS_GIT_SHA` (git rev-parse) + `RUOS_BUILD_DATE` (date -u).
- `kernel/Cargo.toml`: `build = "build.rs"`.
- `kernel/src/boot/{mod,log,banner,error}.rs` (nuovi).
- `boot::log` + macro `binfo!`/`bwarn!`/`berr!` (non ancora chiamate da
  nessuno; T2/T3 le useranno).
- `boot::banner::stamp()` chiamata in `kmain` subito dopo serial init e base revision check.
- `BootError` enum (varianti per ogni phase fail mode).

## Perché

Step 1 del boot refactor: posa l'infrastruttura senza modificare init
flow. kmain ora stampa banner + esegue init come prima. Output esistente
invariato sotto al banner.

## File toccati

- kernel/build.rs (nuovo)
- kernel/Cargo.toml
- kernel/src/boot/mod.rs (nuovo)
- kernel/src/boot/log.rs (nuovo)
- kernel/src/boot/banner.rs (nuovo)
- kernel/src/boot/error.rs (nuovo)
- kernel/src/main.rs
- CHANGELOG/87-26-05-29-boot-refactor-infra.md (nuovo)
