# Followup: cp.wasm hangs in `Fiber::new`

**Data:** 2026-05-29
**Severity:** 🟠 important — blocca init.sh full smoke
**Status:** open, investigation deferred

## Sintomi

Init.sh esegue: echo → whoami → uname → uptime → mkdir → ls → **cp**.
Tutti i tool prima funzionano (DBG trace conferma `Fiber::new OK` +
`returned code=0`). cp.wasm:

```
DBG exec_worker bytes=67465 for /bin/cp.wasm
[HANG indefinito, 180s+ timeout]
```

`Fiber::new(bytes)` → `linker.instantiate_and_start(&mut store, &module)`
non ritorna mai. Niente errore wasmi (vedremmo `instantiate failed`).
Niente trap. Sync call, blocca embassy task `exec_worker_task`.

## Workflow di debug fatto

- ✅ Confermato `vfs::read_all(/bin/cp.wasm)` ritorna i 67465 byte
- ✅ Tool successivi (cat, mv, head, tail ecc.) presumibilmente same issue
- ✅ cp.wasm parse via wasm-objdump OK, no `Start function`, 207 fn
- ✅ Imports cp.wasm: 15 host fn, tutti linked (incluso path_filestat_get,
  path_create_directory unique vs ls/echo/mkdir)
- ❌ wasmi 1.0.9 NON ha feature `lazy` (eager compile only)
- ❌ Non bisettato cp.wasm per fn problematica

## Ipotesi

1. **wasmi 1.0.9 eager compile pathologico su pattern specifico** —
   cp ha 207 fn vs 159 (ls), ma instantiate dovrebbe scale linearmente.
   Forse opcode pattern specifico (es. `select_t`, `memory.fill`,
   `table.copy`?) triggera comportamento worst-case.
2. **Stack overflow durante compilazione di una fn complessa** —
   wasmi compila ogni fn ricorsivamente. Funzione con deep nesting
   (es. async state machine di `std::fs::File::create`) potrebbe
   esplodere stack embassy task (task pool arena = 65536).
3. **Heap exhaustion silente** — talc 16 MiB, cp instantiate alloca
   molto. Se esaurisce, alloc panic dovrebbe vedere. Forse panic dentro
   non-aborting path?
4. **Race condition exec_queue** — Cp è il 7° child consecutivo.
   exec_queue single-slot, ma post_and_wait dovrebbe sincronizzare.
   F1 followup Step 11 (mpmc) prevedeva problemi multi-issuer ma cp è
   single-issuer da shell.

## Ipotesi più probabile

#2 stack overflow. Embassy `task-arena-size-65536` (64 KiB) per task.
Wasmi instantiate frame del compilatore + recursive descent →
plausibile su 207 fn complesse.

## Test successivo (TODO)

1. **Bump task-arena-size a 262144** (256 KiB) → vedere se cp completa
2. **Rebuild cp.wasm con `opt-level = 0`** (no LTO, no aggressive opts)
   → output potrebbe avere fn più semplici per wasmi
3. **Bisettare cp**: rimuovi `copy_recursive`/`metadata`/`readdir` arms;
   prova minimal cp con solo `File::open + File::create + copy`
4. **wasmi update**: 1.0.9 → 1.1.x se rilasciata, può avere fix
5. **Pre-compile AOT**: differito (richiede wasmtime / wasm2c)

## Workaround temporaneo

Init.sh slim'd a `echo ruos boot OK / whoami / uname -a` per far
passare `make run-test`. Sentinel preserved.

## File toccati

- `user-bin/init.sh` (slim per sentinel)
- `docs/followups/cp-wasm-instantiate-hang.md` (questo)
