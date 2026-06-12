# 494 — poll_oneoff sul runtime Wasmtime + prova C/pthread (wasi-sdk)

**Data:** 2026-06-12

## Cosa

- **`poll_oneoff` nello shim WASI wt** (`wt/wasi.rs`): clock subscription only
  (ciò che emettono `std::thread::sleep` e `usleep`/`nanosleep` C), prima
  subscription onorata, non-clock → EINVAL — speculare allo shim wasmi
  (host/lifecycle.rs). Strategia di attesa: su un FIBER wasm-thread il sleep
  **parcheggia il fiber** (`threads::sleep_current`, nuovo: park con chiave =
  il fiber stesso, riscatto via `expire_timeouts` come i timeout futex — il
  core resta libero); sul path `.cwasm` sync classico hlt-wait sul posto
  (documentato: un tool CLI che dorme conviene shipparlo `.wasm`/wasmi sui
  sistemi 1-2 core). Prima un import `poll_oneoff` faceva fallire
  l'instantiate: nessun programma con sleep/timeout poteva girare su wt.
- **`tools/hello-pthread/`** (nuovo): C compilato con **wasi-sdk 24**
  (`--target=wasm32-wasip1-threads -pthread`) — pthread_create + join +
  usleep nel thread E nel main. Artefatto `.wasm` VENDORED committato (come
  lua.wasm; wasi-sdk non è dipendenza di `make iso`), `wt-precompile` →
  `/bin/hello-pthread.cwasm`. **Gotcha documentato**: wasi-sdk NON passa
  `--import-memory` di default (Rust sì) → senza
  `-Wl,--import-memory,--export-memory,--max-memory=N` il modulo definisce
  una memoria shared PROPRIA, ogni thread avrebbe la sua, e il router .cwasm
  non riconosce il modulo come threaded.
- mtstress: aggiunto `std::thread::sleep(50ms)` (copre il path Rust di
  poll_oneoff su fiber). `threads-init.sh` + `tests/threads-test.sh`: nuovo
  assert `PTHREAD_C_OK val=42 ret=123`.
- `docs/api/wasi.md`: entry poll_oneoff wt + ricetta wasi-sdk per moduli
  threaded C. `CLAUDE.md`: wasi-sdk in toolchain.

## Perché

Quasi ogni programma MT reale usa sleep/timeout: senza poll_oneoff il
runtime threaded era limitato a workload puro-compute. La prova C chiude
anche la domanda "gira software C/C++ che chiede il MT?": sì — stesso ABI
wasi-threads di Rust, via wasi-sdk coi flag giusti.

## Verifica

- `PARSUM_OK threads=4`, `STRESS_MT_OK count=400000` (ora CON sleep),
  `PTHREAD_C_OK val=42 ret=123`, `THREADS_INIT_DONE` su QEMU -smp 6.
- Gate boot-checks: 4/4 marker ok su -smp 4 E -smp 1.
- `make run-test`: TEST_PASS.
- **HARDWARE REALE** (verifica manuale Giuseppe, 2026-06-12/13): sistema
  fluido coi wasm-thread attivi — chiude lo step CPU-sensitive del piano
  (più forte del VBox richiesto).

## File toccati

- kernel/src/wasm/wt/wasi.rs
- kernel/src/wasm/wt/threads.rs
- tools/hello-pthread/ (nuovo: .c + build-wasm.sh + .wasm vendored)
- tools/mtstress/src/main.rs
- user-bin/threads-init.sh
- tests/threads-test.sh
- Makefile
- docs/api/wasi.md
- CLAUDE.md
- CHANGELOG/494-26-06-12-wt-poll-oneoff-c-pthread.md
