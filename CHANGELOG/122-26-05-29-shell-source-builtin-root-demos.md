# 122 — shell `source` builtin + move server/client to /root/

**Data:** 2026-05-29

## Cosa

1. **`source <path>` builtin** in shell (alias `. <path>`). Legge file,
   itera linee (skip vuote + commenti `#`) e ognuna passa per `run_command`.
   Stesso loop di `/etc/init.sh` a boot. Ricorsivo.
2. **Move demo blobs** `server.wasm` + `client.wasm` da `/` a `/root/`:
   - `Makefile`: split `ROOT_WASMS` (solo `init.wasm`) e nuovo `ROOT_DEMOS`
     (server+client). `mkdir -p $(ISO_ROOT)/root`. cp loop per `ROOT_DEMOS`
     in `/root/`. Sia `iso:` che `test-boot:` aggiornati.
   - `limine.conf`: `module_path` aggiornati a `/root/server.wasm`,
     `/root/client.wasm`.
   - `kernel/src/wasm/mod.rs`: match arms pre-open socket per i nuovi path
     (`/root/server.wasm` listen 8080, `/root/client.wasm` connect).
   - `kernel/src/executor/mod.rs`: commento aggiornato.

`init.wasm` resta a `/` per ora (non specificato dall'utente come da
spostare). `etc/init.sh` resta a `/etc/` (richiesto).

## Perché

- Shell ora puo' eseguire script ad-hoc, non solo init.sh a boot.
- `/` non era pulito: ospitava 3 blob demo. Convenzione UNIX `/root/`
  per home root. ls / mostra meno rumore.

## File toccati

- user/shell/src/main.rs
- Makefile (ROOT_DEMOS split + mkdir /root)
- limine.conf
- kernel/src/wasm/mod.rs
- kernel/src/executor/mod.rs
- CHANGELOG/122-26-05-29-shell-source-builtin-root-demos.md (questo)

## Test

`make iso && make run-test` → TEST_PASS. Init.sh completa al solito,
sentinel `shell: init.sh complete` fires. Demo `/root/server.wasm` +
`/root/client.wasm` invocabili dallo shell con path completo.
