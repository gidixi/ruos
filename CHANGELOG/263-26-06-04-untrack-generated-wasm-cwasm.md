# 263 — Stop tracking generated wasm/cwasm + ignore build debris

**Data:** 2026-06-04

## Cosa
Tolti dal versionamento ~1.5 MB di binari rigenerabili:
- `user-bin/*.wasm` (56 tool) — `git rm --cached`; ricostruiti dalla pattern
  rule `make` esistente da sorgenti in `user/`.
- `kernel/src/wasm/wt/*.cwasm` (demo AOT boot-check) — `git rm --cached`; ora
  rigenerati nel source tree da nuove rule Makefile (`make wt-cwasm`, anche
  prereq di `test-boot`). `wt-precompile` ha la feature `wat` per compilare
  hello/gfxtest direttamente dai `.wat`.
- `.gitignore`: aggiunti `*.log`, `*.ppm`, `*.png`, `/user/target/`,
  `/tools/*/target/`, `user-bin/*.wasm`, `kernel/src/wasm/wt/*.cwasm`.

## Perché
Il working tree mostrava churn binario costante (rebuild dei .wasm) e i .cwasm
(~760 KB) gonfiavano il repo. Tutti rigenerabili da sorgente → non vanno
"portati dietro".

## File toccati
- .gitignore
- Makefile
- tools/wt-precompile/Cargo.toml, tools/wt-precompile/Cargo.lock
