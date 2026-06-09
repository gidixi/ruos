# 380 — cwd come stato del guest: path relativi onorano la working dir

**Data:** 2026-06-09

## Cosa

Fix definitivo del limite noto di #367: i tool che aprono path **relativi alla
cwd** (es. `cat foo.txt` mentre lo shell è in `/etc`) ora li risolvono contro la
working directory, non da `/`. La cwd diventa **stato del guest** (libc), con il
kernel che resta stateless sui path (`resolve_at` base `/`).

Catena:
1. **Kernel inietta `PWD=<cwd>`** nell'environ di ogni figlio
   (`kernel/src/wasm/fiber.rs::set_cwd`): rimpiazza/aggiunge `PWD` in
   `RuntimeState.env` quando la cwd è settata sul figlio.
2. **WASI environ implementato** (`kernel/src/wasm/host/lifecycle.rs`):
   `environ_sizes_get`/`environ_get` erano stub che ritornavano 0 env var. Ora
   espongono `RuntimeState.env` (mirror di `args_get`), così il guest vede `PWD`.
3. **Nuovo crate `user/ruos-rt`**: `ruos_rt::init()` legge `PWD` e chiama
   `std::env::set_current_dir` → aggiorna `__wasilibc_cwd`. Va chiamato come prima
   riga di `main` (un ctor `.init_array` in un crate-dipendenza NON si linka senza
   referenza — verificato con spike).
4. **`ruos_rt::init()` aggiunto** a shell + 22 tool std::fs (cat, cp, cut, diff,
   du, find, grep, head, init, mkdir, mv, nano, nc, rm, rmdir, sort, tail, tee,
   touch, uniq, wc, wget, which). `ls` non serve (usa la host fn custom `readdir`
   → già cwd-aware via kernel cwd). Tool senza I/O su file non toccati.
5. **Shell `cd`**: oltre al kernel `chdir`, chiama `set_current_dir` locale così
   anche le sue op relative (`source foo.sh`) onorano la nuova dir.

## Perché

Con kernel `resolve_at` base `/` (#367), wasi-libc rootava i path relativi sulla
PROPRIA cwd sempre "/", perdendo la cwd reale. Distinguere assoluto da relativo
lato kernel è impossibile (wasi-libc strippa il leading slash di entrambi).
L'unico fix corretto: la cwd vive nella libc del guest, sincronizzata via `PWD`.
Così libc rootea il relativo sulla cwd VERA *prima* di strippare → il kernel
(base `/`) lo vede già giusto. Assoluto e relativo entrambi corretti.

## Spike (pre-impl)

Verificato su `wasm32-wasip1`: (a) `set_current_dir` aggiorna `__wasilibc_cwd` e i
path relativi lo usano; (b) i ctor `.init_array` girano prima di `main`; (c) un
ctor in crate-dipendenza NON si linka senza referenza → da qui la scelta di
`ruos_rt::init()` esplicito in `main`.

## Verifica

Boot headless, init `cd /etc ; wc init.sh ; grep cd init.sh ; head -n 1 init.sh`
(tutti path RELATIVI a cwd /etc):
- `wc init.sh` → `4 11 53 init.sh`
- `grep cd init.sh` → match `cd /etc`
- `head -n 1 init.sh` → `cd /etc`
Tutti leggono `/etc/init.sh`. Path assoluti continuano a funzionare; `cat` su path
relativo idem.

## File toccati
- kernel/src/wasm/fiber.rs (set_cwd → inietta PWD)
- kernel/src/wasm/host/lifecycle.rs (environ_get/environ_sizes_get reali)
- kernel/src/wasm/host/path.rs (commento aggiornato)
- user/ruos-rt/ (nuovo crate)
- user/Cargo.toml (membro ruos-rt)
- user/shell + 22 tool std::fs (Cargo.toml dep + ruos_rt::init())
