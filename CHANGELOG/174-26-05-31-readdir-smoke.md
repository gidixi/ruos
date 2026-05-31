# 174 — WASI fd_readdir Task 4: smoke test + bugfix risoluzione dir_fd

**Data:** 2026-05-31

## Cosa
Smoke test end-to-end che prova `fd_readdir` via la libreria standard, e
il bugfix scoperto facendolo passare.

- `user/readdirtest/`: nuovo crate `wasm32-wasip1` che chiama
  `std::fs::read_dir(<dir>)` (default `/bin`), conta le entry e stampa
  `readdir-std: <N> entries`. Usa la via **standard** (non `ruos.readdir`),
  quindi esercita davvero `path_open(O_DIRECTORY)` + `fd_readdir`.
- `user/Cargo.toml`, `Makefile` (BIN_TOOLS), `limine.conf`: wiring del
  nuovo `.wasm`.
- `user-bin/smoke.sh`: aggiunto `readdirtest /bin`.
- `Makefile` `run-test`: grep `readdir-std: [1-9][0-9]* entries` →
  `TEST_FAIL_READDIR` se manca.

### Bugfix (il pezzo critico)
`std::fs::read_dir` falliva con `0 entries` e un `Err(ENOENT 44)`. Due
cause, scoperte tracciando le host call:

1. **fd preopen aliasing**: l'handler `OpenDir` allocava il fd partendo
   da `skip(3)`, beccando lo slot 3 = fd 3, che è il **preopen root "/"**
   virtuale di WASI. std lo usa come base per risolvere i path →
   corruzione. Fix: `OpenDir` alloca fd `>= 4`
   (`kernel/src/wasm/fiber.rs`).
2. **risoluzione path relativa a dir_fd** (`kernel/src/wasm/host/path.rs`):
   dopo `fd_readdir`, std chiama `path_filestat_get(dir_fd=<fd di /bin>,
   "<entry>")` per ogni voce. `read_path`/`path_open` **ignoravano
   `dir_fd`** e risolvevano contro la cwd ("/") → cercavano `/cat.wasm`
   invece di `/bin/cat.wasm` → ENOENT. Aggiunto helper `resolve_at(caller,
   dir_fd, path)`: se `dir_fd` è un `FdEntry::Dir(base)` risolve contro
   `base`, altrimenti contro la cwd (preopen fd 3 / fd non-dir →
   comportamento precedente, nessuna regressione su file assoluti/cwd).
   Applicato a `path_open`, `path_filestat_get`, `path_unlink_file`,
   `path_create_directory`, `path_remove_directory`, `path_rename`.

## Perché
La spec assumeva "path_open ignora dir_fd, risolve contro cwd" — vero
per i tool esistenti (path assoluti), ma `std::fs::read_dir` risolve le
entry contro il dir_fd: senza dir_fd-awareness ogni stat falliva.

## Test
- Probe isolato: `readdir-std: 45 entries` (44 tool + readdirtest, `.`/`..`
  filtrati da std) = coerente con `ls /bin`.
- `make run-test` → TEST_PASS (con il nuovo gate readdir).
- `make run-ssh-test` → TEST_PASS_SSH (nessuna regressione).

## File toccati
- user/readdirtest/Cargo.toml, user/readdirtest/src/main.rs
- user/Cargo.toml
- Makefile
- limine.conf
- user-bin/smoke.sh
- kernel/src/wasm/fiber.rs (OpenDir fd>=4)
- kernel/src/wasm/host/path.rs (resolve_at + dir_fd-aware)
