# 108 — Process registry + cooperative kill

**Data:** 2026-05-29

## Cosa
Nuovo modulo `kernel/src/proc.rs` con registry `BTreeMap<u32, ProcInfo>`
(pid, name, start_tick, kill flag). `Fiber` ora ha un campo `pid:
Option<u32>`; `Fiber::set_pid` lo imposta; il run loop dopo ogni
dispatch controlla `proc::is_kill_pending(pid)` e termina con exit code
137 (convenzione SIGKILL) se settato.

`wasm::run_at` (per i .wasm top-level: init/server/client/shell) e
`executor::exec_worker_task` (per i child via `ruos_exec`) ora
chiamano `proc::register(name)` → set_pid → run → `proc::unregister`.

`request_kill(pid)` flippa il flag (no signal model); `list()` clona
gli entry per `ruos_proc_list`.

## Perché
`ps` e `kill` userspace richiedono visibilità sui fiber attivi e un
modo per terminarli. Niente segnali POSIX perché l'executor è
cooperativo: la fiber viene chiusa al successivo punto di suspend
host-fn (read/write/sleep/exec/etc.) — frequente abbastanza da essere
percepito istantaneo.

## File toccati
- kernel/src/proc.rs (nuovo)
- kernel/src/wasm/fiber.rs
- kernel/src/wasm/mod.rs
- kernel/src/executor/mod.rs
- kernel/src/main.rs
