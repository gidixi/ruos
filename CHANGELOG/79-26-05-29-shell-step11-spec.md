# 79 — Spec design: Shell Step 11 (shell.wasm + ruos_exec)

**Data:** 2026-05-29

## Cosa

Scritta spec Step 11 in
`docs/superpowers/specs/2026-05-29-rust-shell-step11-design.md`.

Decisioni strategiche:

- **Opt B**: shell come `shell.wasm` (`wasm32-wasip1`), non kernel-side.
  Coerenza WASIX-first.
- **Full scope**: 4 builtin (`cd`, `pwd`, `exit`, `help`) + 3 external
  (`ls`, `cat`, `echo`) + `/etc/init.sh` boot script.
- Smoke contract: `shell: init.sh complete`.

Componenti:
- Nuovi host fns: `ruos_exec(path, argv, exit_code_ptr) -> errno` e
  `ruos_readdir(path, buf, buf_len, nread_ptr) -> errno`.
- Nuove `SuspendReason::{Exec, ReadDir}`.
- `Fiber::set_args` + `lifecycle::args_*` real (sostituisce zero stub).
- 4 nuovi user crates: shell, ls, cat, echo.
- 5 nuovi moduli Limine: init.sh + 4 .wasm.

Drop `kbd_echo_task` (F7 Step 10.5 fix): shell.wasm unico consumer
keyboard. Init.wasm/server.wasm/client.wasm restano auto-spawnati come
Step 10 baseline (decision B nello spec).

Decomposizione 3 task:
1. Host fns + lifecycle args reali.
2. External tools (4 user crates) + shell boot-script path + init.sh.
3. Drop kbd_echo_task + final regression.

Out of scope: pipes, redirect, jobs, history, env vars, globbing,
signals, bash.wasm upgrade.

## Perché

Step 11 della roadmap. Sblocca userland interattivo (manuale QEMU) +
demonstrate WASIX architecture end-to-end con shell + tool reali.

## File toccati

- docs/superpowers/specs/2026-05-29-rust-shell-step11-design.md (nuovo)
- CHANGELOG/79-26-05-29-shell-step11-spec.md (nuovo)
