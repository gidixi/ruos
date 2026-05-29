# 112 — run-test timeout 30s → 120s

**Data:** 2026-05-29

## Cosa

Bump `make run-test` timeout 30s → 120s.

## Perché

Init.sh esegue ~22 comandi wasm. Ogni `Fiber::new(bytes)` =
wasmi 1.0.9 eager compile ~2-3s su modulo 50-80 KB. Full script
~50s in QEMU TCG.

Timeout 30s troncava a cmd #4-5 (uptime). Sembrava cp.wasm
hang in `Fiber::new` ma era solo "non ancora arrivato".

Conferma: full init.sh run con timeout 120s → completa T+53s,
sentinel `shell: init.sh complete` fires, tutti i 22 coreutils
(echo/whoami/uname/uptime/mkdir/ls/cp/cat/mv/head/tail/du/grep/
find/rm/free/df/lscpu/ps) OK.

## File toccati

- Makefile (run-test target)
- user-bin/init.sh (ripristinato a full smoke da CHANGELOG/111)
- docs/followups/cp-wasm-instantiate-hang.md (rimosso — non-bug)
