# 379 — fix risoluzione path: tool esterni + `cd ..` a cwd ≠ "/"

**Data:** 2026-06-09

## Cosa

Due bug della working-directory nello shell, stessa area (gestione cwd /
trailing-slash):

**Bug A — nessun comando esterno parte a cwd ≠ "/".** A `/bin`, `ls` dava
`shell: ls: not found`. Causa: `kernel/src/wasm/host/path.rs::resolve_at`
risolveva i path del preopen fd 3 contro la **cwd del kernel**. Ma wasi-libc
risolve contro il preopen "/" e passa al kernel il path SENZA il leading slash
(`/bin/ls.wasm` → `bin/ls.wasm`); ri-applicando la cwd si raddoppiava: a `/bin`,
`bin/ls.wasm` → `/bin/bin/ls.wasm` → ENOENT. Rompeva ogni comando esterno e ogni
path assoluto a qualunque cwd ≠ "/" (a "/" funzionava perché cwd == "/"). Fix:
`resolve_at` per il preopen/fd non-dir usa `base = "/"` (i path arrivano già
rootati a "/"). I Dir-fd (`std::fs::read_dir`) restano risolti contro la dir.

**Bug B — `cd ..` da una dir con trailing slash sbagliava il prompt.** `cd bin/`
lasciava il mirror locale `CWD = "/bin/"`; poi `cd ..` faceva `rfind('/')` sullo
slash finale → `"/bin"` invece di `"/"` (il prompt divergeva dalla cwd reale del
kernel, che era corretta). Fix: `user/shell/src/main.rs::builtin_cd` ora
normalizza il mirror con `norm_cwd`, che ricalca `resolve_cwd` del kernel
(split su '/', drop ""/".", pop su "..", re-root "/", niente trailing slash).

## Perché

A cwd ≠ "/" lo shell era inutilizzabile (nessun tool eseguibile) e il prompt
mentiva dopo `cd ..`. Entrambi derivavano da una gestione errata di cwd/slash.

## Limite noto

Con `base = "/"`, un tool che apre un path **genuinamente relativo alla cwd**
(es. `cat foo.txt` mentre lo shell è in `/mnt`) lo risolve da "/", non dalla cwd:
wasi-libc ha la propria cwd sempre "/" e, perso il leading slash, non si distingue
più un path originariamente assoluto da uno relativo. Strict improvement comunque
(prima a cwd ≠ "/" non partiva NULLA). Fix proprio futuro: sincronizzare la cwd
libc del guest con la cwd del kernel all'avvio del processo. Per ora: usare path
assoluti come argomenti dei tool.

## Verifica

Boot headless con init `cd bin/ ; pwd ; ls /bin ; cd .. ; pwd ; ls`:
- `pwd` dopo `cd bin/` → `/bin`
- `ls /bin` → elenca i tool (prima: `ls: not found`)
- `pwd` dopo `cd ..` → `/` (prima: `/bin`)
- nessun `not found`.

## File toccati
- kernel/src/wasm/host/path.rs
- user/shell/src/main.rs
