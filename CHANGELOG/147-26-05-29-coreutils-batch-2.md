# 147 — coreutils batch 2: touch, wc, clear, which, sort, uniq, cut, tr, tee

**Data:** 2026-05-29

## Cosa

9 nuovi wasm tool userland (~20-80 LoC ciascuno):

- **touch** `<file>...` → create empty (CREATE | WRITE)
- **wc** [-l/-w/-c] `<file>`/stdin → lines/words/bytes; `total` se ≥2
- **clear** → `\x1b[2J\x1b[H` su stdout
- **which** `<cmd>...` → prima match in `/bin/<cmd>.wasm` o `/usr/bin/`
- **sort** [-r/-u] `<file>`/stdin → lessicografico, opzioni reverse/unique
- **uniq** [-c] `<file>`/stdin → squash adjacent duplicates, `-c` count
- **cut** [-d delim] [-f field-list] [-c char-range] → split + select
- **tr** SET1 SET2 / -d SET → char map o delete
- **tee** [-a] `<file>...` → stdin → stdout + files

Tutti registrati in `user/Cargo.toml`, `Makefile BIN_TOOLS`,
`limine.conf`.

## Test

`make run-test` → TEST_PASS (no regression). Disponibili a shell:
```
ruos:/$ touch /tmp/x && wc /tmp/x
ruos:/$ ls / | sort
ruos:/$ which cat
ruos:/$ which cp
```

## File toccati

- user/{touch,wc,clear,which,sort,uniq,cut,tr,tee}/Cargo.toml + src/main.rs (nuovi)
- user/Cargo.toml (members)
- Makefile (BIN_TOOLS)
- limine.conf (module_path)
- CHANGELOG/147-26-05-29-coreutils-batch-2.md (questo)
