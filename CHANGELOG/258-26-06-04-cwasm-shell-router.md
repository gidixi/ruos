# 258 — Router shell `.cwasm` → Wasmtime + pipeline Makefile

**Data:** 2026-06-04

## Cosa
Integrato il runtime Wasmtime nel flusso di esecuzione comandi:
- **Shell** (`user/shell`): `resolve_path` e la tab-completion ora considerano sia
  `.wasm` (wasmi) sia `.cwasm` (Wasmtime AOT), in `/bin` poi `/mnt/bin`.
- **exec_worker_task** (`executor/mod.rs`): router per estensione — `.cwasm` →
  `crate::wasm::wt::run_cwasm`, `.wasm` → wasmi Fiber (invariato).
- **Makefile**: rule `wt-precompile` + `build/wtecho.cwasm` (da echo.wasm); l'ISO
  stagia `/bin/wtecho.cwasm`. `limine.conf`: modulo wtecho.cwasm.
- Init di test `user-bin/wt-router-init.sh` (esegue `wtecho ROUTER-OK`).

Verificato in QEMU: la shell esegue `wtecho` → `/bin/wtecho.cwasm` → router →
Wasmtime → stampa `ROUTER-OK`; `echo` (wasmi) continua a funzionare (`ruos boot
OK`). Due runtime coesistono, scelti per estensione.

## Perché
Rende i `.cwasm` lanciabili dalla shell come i tool normali — il path con cui la
GUI (`gui.cwasm`) verrà avviata. Completa il grosso del piano #7. Nota: lo stdout
dei `.cwasm` va a CONSOLE (serial+fb); il binding a PTY per pipeline/SSH è
follow-up.

## File toccati
- user/shell/src/main.rs, user-bin/shell.wasm
- kernel/src/executor/mod.rs
- Makefile, limine.conf
- user-bin/wt-router-init.sh
