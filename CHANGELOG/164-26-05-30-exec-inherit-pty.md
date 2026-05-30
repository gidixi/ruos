# 164 — ruos_exec: eredita la PTY del caller (fix SSH output → fb)

**Data:** 2026-05-30

## Cosa
Il path single-command (`ruos_exec`) ora propaga `term_pts` dal caller
al figlio, come già fa `ruos_exec_pipeline` da Step 12 (pipes).

Catena modificata:
- `kernel/src/wasm/host/proc.rs::ruos_exec` calcola `term_pts` da
  `caller.data().fds[1]` (= `pts_index` se è uno slave PTY, altrimenti 0).
- `kernel/src/wasm/suspend.rs::SuspendReason::Exec` ora porta
  `term_pts: usize`.
- `kernel/src/wasm/exec_queue.rs::{ExecSlot, ExecFuture, post_and_wait}`
  estesi con `term_pts`.
- `kernel/src/wasm/fiber.rs::dispatch(Exec)` inoltra `term_pts`.
- `kernel/src/executor/mod.rs::exec_worker_task` chiama
  `child.rebind_stdio_pty(slot.term_pts)` prima di `child.run()`.

Aggiunti diagnostic log:
- `INFO ssh rebind_stdio_pty idx=N bound stdin=B stdout=B stderr=B` —
  conferma riuscita del rebind dopo apertura di `/dev/pts/<N>`.
- `INFO pipe exec_pipeline stages=N term_pts=N (src)` — quale PTY
  eredita il pipeline e da dove l'ha dedotto.

## Perché — il bug catturato dai serial log VBox dell'utente
Dopo aver loggato in SSH e digitato `ls`:

```
INFO ssh shell started on pty 1
INFO ssh rebind_stdio_pty idx=1 bound stdin=true stdout=true stderr=true   ← rebind dello shell OK
DIR  0 bin/          ← output di ls sul framebuffer/seriale (pty 0)
DIR  0 dev/
...
INFO ssh session done (rx=8 tx=177 sent=1386 txdrop=0)   ← solo 177 byte (banner shell) usciti via SSH
```

Lo shell SSH **era** correttamente legato a `/dev/pts/1`, MA il bridge
PTY → SSH vedeva soltanto i byte scritti **dallo shell stesso** (banner,
prompt). Quando l'utente lanciava `ls`, lo shell chiamava `exec` (single
command path), che NON ereditava il `term_pts` del caller (a differenza
di `exec_pipeline`, fixato già per le pipes in Step 12). Il figlio
`ls.wasm` veniva istanziato con stdio sui default `/dev/pts/0` →
output drenato dal `console_drain_task` al framebuffer → seriale → "lo
vedi su VBox, non in SSH".

Il sintomo riportato — "vengono generate più istanze shell ma tutte
scrivono sulla buffer del server" — era esatto: la istanza shell era su
pty 1, ma i suoi *figli* (ls, pwd, date, ip, ps, ...) ricadevano su
pty 0. Lo conferma anche il match `tx=177` = solo il banner "ruos shell
ready" + prompts, senza l'output dei comandi.

## Note
- I test `make run-ssh-test`/`run-passwd-test`/`run-passwd-diskless-test`
  passavano già prima del fix perché eseguono `ssh host pwd` come exec
  remoto SSH: il server seedava la richiesta scrivendo `pwd\n` nel PTY
  master input (`pty::master_input_push(idx, …)`) per fare girare il
  ciclo shell interno, ma il `pwd` builtin del shell **scrive direttamente
  su stdout** senza passare per `ruos_exec` — quindi era servito da
  pty 1 correttamente. Solo i comandi `.wasm` esterni cadevano sul path
  buggato. Servirebbe un test apposito che esegua un comando *esterno*
  in SSH interattivo (es. `ls`) per coprire il path; lo lascio come
  TODO follow-up.
- `tx_dropped` rimane (sempre 0) nel log session: tenuto per
  compat dei consumer di log.

## File toccati
- kernel/src/wasm/host/proc.rs
- kernel/src/wasm/suspend.rs
- kernel/src/wasm/exec_queue.rs
- kernel/src/wasm/fiber.rs
- kernel/src/executor/mod.rs
