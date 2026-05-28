# 61 — Limine modules → VFS mount (Step 10 Task 1)

**Data:** 2026-05-28

## Cosa

- `limine.conf` dichiara `module_path: boot():/init.wasm` con
  `module_cmdline: /init.wasm`.
- `Makefile` include `user-bin/init.wasm` nel root dell'ISO.
- `kernel/src/modules.rs` (nuovo): `ModulesRequest` static, `mount_all()`
  itera i moduli e copia ciascuno in tmpfs al `module_cmdline` come path.
- `kmain` chiama `modules::mount_all()` dopo `vfs::init`.
- `user-bin/init.wasm` placeholder 1 byte; Task 2 lo sostituisce con
  bytecode reale.

## Perché

Primo task dello Step 10 (WASIX bootstrap). Standalone-loadable
boot modules sbloccano i prossimi 5 task: i `.wasm` arrivano al
kernel come file VFS, niente embedding.

## File toccati

- limine.conf
- Makefile
- kernel/src/modules.rs (nuovo)
- kernel/src/main.rs
- user-bin/init.wasm (placeholder 1 byte)
- CHANGELOG/61-26-05-28-wasix-limine-modules.md (nuovo)
