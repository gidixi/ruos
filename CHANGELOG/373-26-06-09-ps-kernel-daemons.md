# 373 — demoni kernel visibili in `ps` (stile [kthread])

**Data:** 2026-06-09

## Cosa

`ps` ora elenca anche i demoni async kernel, con nome `[bracketed]` (convenzione
kernel-thread Linux), oltre alle fiber wasm.

- `kernel/src/proc.rs`: `ProcInfo` ha un nuovo campo `kernel: bool`. Aggiunta
  `register_kernel(name)` che inserisce nel registry con `kernel=true` e nome
  `[name]`. `request_kill` ora **rifiuta** i processi kernel (non si killa il
  watchdog/sshd) → ritorna false (ESRCH lato tool).
- `kernel/src/executor/mod.rs`: ogni task `#[embassy_executor::task]` di lunga
  vita si auto-registra all'entry: `init` (boot_shell respawn), `sshd`
  (ssh_serve), `pty-dispatch`, `svc-dispatch`, `exec-worker`, `pipe-worker`,
  `console-drain`, `watchdog`. (Il `bin_overlay_task` one-shot resta escluso.)

Nessun cambio al formato wire di `ruos_proc_list`/`proc_stat` né ai tool `ps`/
`rtop`: il bracketing è kernel-side, i tool stampano il nome verbatim.

## Perché

Prima `ps` mostrava solo le fiber wasm registrate in `proc::REGISTRY` (shell, app
GUI, tool); i demoni kernel (watchdog, sshd, dispatcher, worker) erano task
dell'executor cooperativo senza pid → invisibili. Domanda dell'utente: "perché non
vedo il watchdog in ps?". Ora sono visibili (e protetti da kill), come i
`[kworker]`/`[ksoftirqd]` su Linux.

## Verifica

Boot headless con init che lancia `ps`:

```
  PID    ELAPSED CMD
    1       0.08 [svc-dispatch]
    2       0.08 [watchdog]
    3       0.08 [pty-dispatch]
    4       0.08 [sshd]
    5       0.08 [pipe-worker]
    6       0.08 [exec-worker]
    7       0.08 [init]
    8       0.02 bin/shell.wasm
    9       0.01 [console-drain]
```

`make test-boot` → `TEST_BOOT_PASS` (nessun panic dal cambio registry).

## File toccati
- kernel/src/proc.rs
- kernel/src/executor/mod.rs
