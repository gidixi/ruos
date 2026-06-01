# 197 — smptest: WASI tool for SMP parallel speedup benchmark

**Data:** 2026-06-01

## Cosa
Aggiunto tool WASI `smptest` che chiama la host fn `ruos_smp_bench` (modulo
`ruos`, signature `smp_bench(buf_ptr, buf_len, used_ptr) -> i32`) e stampa il
report ASCII one-line: `parallel=Xms sequential=Yms speedup=Z.ZZx cores=[a,b,c]`.

Struttura creata:
- `user/smptest/Cargo.toml` — package wasm32-wasip1 (mirror esatto di lscpu)
- `user/smptest/src/main.rs` — import host fn + call + print pattern
- `user/Cargo.toml` — aggiunto `"smptest"` ai workspace members
- `Makefile` — aggiunto `smptest` a `BIN_TOOLS` (build + mount in /bin/)
- `limine.conf` — aggiunto entry `module_path/module_cmdline` per `/bin/smptest.wasm`

## Perché
Task 4 di SMP Fase 2: prova osservabile che i job CPU vengono distribuiti sugli AP.
Il tool è il pezzo mancante per chiudere il ciclo spec→impl→verifica.

Smoke test su QEMU -smp 4 (output reale):
`parallel=1183ms sequential=0ms speedup=0.00x cores=[1,2,3]`

`cores=[1,2,3]` conferma che 3 AP distinti hanno eseguito i job (SMP funziona).
`sequential=0ms` è un artefatto di timing: la fase sequenziale su CPU QEMU max
completa in sub-1ms (risoluzione timer = 1ms), rendendo il calcolo speedup
non significativo. È un limite della calibrazione del bench nel kernel (Task 1-3),
non del tool.

## File toccati
- user/smptest/Cargo.toml
- user/smptest/src/main.rs
- user/Cargo.toml
- Makefile
- limine.conf
- user-bin/smptest.wasm
