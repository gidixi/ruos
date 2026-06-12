# 493 — MT Fase 2 Task 7: mtstress + kill-group su trap + docs di fase

**Data:** 2026-06-12

## Cosa

- **`tools/mtstress/`** (nuovo): 4 thread incrementano un contatore sotto un
  `std::sync::Mutex` conteso (futex wait/notify reali) + join; valore ESATTO
  `STRESS_MT_OK count=400000` = prova di atomicità e coerenza. Variante
  `mtstress trap`: un thread fa `std::process::abort()` (trap `unreachable`).
- **Kill-group su trap** (`wt/threads.rs`): un thread che trappa avvelena il
  gruppo (`poisoned`) e l'intero gruppo muore con exit 134 —
  (a) i fiber runnable muoiono al take in `run_one`;
  (b) i parcheggiati li rimuove **`kill_group_waiters`** (scan WAITQ per
  `Arc::ptr_eq`, drop del fiber sospeso: stack liberato, Store dentro muore
  senza Drop — accettato, il gruppo è morto), con `live`/`ps`/timed-counter
  aggiornati via `finish_fiber`;
  (c) gli in-esecuzione muoiono al prossimo park (check `poisoned` nel branch
  Err di `run_one`). Il kernel NON panica, la shell sopravvive.
- **`tests/threads-test.sh`**: assert `STRESS_MT_OK count=400000`, assert che
  `UNREACHABLE` non compaia (il join dopo il trap non riprende MAI) e che la
  shell stampi `THREADS_INIT_DONE` dopo il `mtstress trap`.
- **Docs di fase**: spec `2026-06-12-wasm-mt-fase2-threads-design.md` → stato
  IMPLEMENTATO + §13 esiti e deviazioni (no async_support → fiber nostri;
  NO_DEADLINE per i thread store; TIMED_WAITERS; protocollo crediti);
  `CLAUDE.md` (toolchain `wasm32-wasip1-threads` + sezione due-runtime);
  `build-iso.ps1` (rustup target).

## Perché

MT Fase 2 Task 7 (chiusura fase): stress di contesa reale + la garanzia di
contenimento — un'app threaded che trappa muore PULITA (niente fiber zombie
in WAITQ, niente core bruciati, niente panic kernel) — e la documentazione
della fase allineata a ciò che è stato costruito.

## Regressione

- `make run-test`: TEST_PASS. `tests/threads-test.sh`: tutti i gate smp4+smp1
  + `PARSUM_OK threads=4` + `STRESS_MT_OK count=400000` + trap/kill-group ok.
- **`tests/frame-smp-test.sh`: FAIL PRE-ESISTENTE** (attribuzione VERIFICATA:
  identico esito — marker `frame cores=` mai emesso + `frame() WATCHDOG …
  'shell': killed` a T+9s — anche al commit pre-fase 6ff2dd8, ribuildato e
  bootato apposta). NON è una regressione della fase 2; il test non è wired
  nel Makefile e non risulta eseguito dai changelog ~476 in poi. Da
  investigare A PARTE: il watchdog epoch uccide la finestra shell sotto
  compositor-init e FRAME_JOBS non raggiunge mai ≥2 a frame ≥30.
- VBox (CPU-sensitive, OBBLIGATORIO da piano): verifica manuale utente
  ancora da fare — ISO boot-checks + threads-init, ≥4 vCPU.

## File toccati

- tools/mtstress/ (nuovo)
- kernel/src/wasm/wt/threads.rs
- user-bin/threads-init.sh
- tests/threads-test.sh
- Makefile
- docs/superpowers/specs/2026-06-12-wasm-mt-fase2-threads-design.md
- CLAUDE.md
- build-iso.ps1
- docs/superpowers/plans/2026-06-12-wasm-mt-fase2-threads.md
- docs/superpowers/plans/2026-06-12-wasm-mt-fase2-task2-fiber-runtime.md
- CHANGELOG/493-26-06-12-wt-mtstress-killgroup-docs.md
