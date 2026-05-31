# 170 — boot shell respawn: la console locale non muore più su `exit`

**Data:** 2026-05-31

## Cosa
La shell della console locale (pts/0, framebuffer + seriale) ora viene
**ri-spawnata** quando esce, invece di lasciare la console morta.

- `kernel/src/wasm/mod.rs`: nuova `run_boot_shell(replay_init: bool) -> i32`.
  Lancia `/bin/shell.wasm` su pts/0; `replay_init=true` (solo al primo
  avvio) fa rieseguire `/etc/init.sh`, `false` (ogni respawn) salta
  l'init e dà un prompt pulito (`--no-init`, stesso flag delle shell SSH).
- `kernel/src/executor/mod.rs`: nuovo `boot_shell_task` che fa loop su
  `run_boot_shell`, ri-spawnando dopo ogni uscita con un guard di 200 ms
  contro crash-loop stretti. Sostituisce lo spawn singolo di
  `wasm_task("/bin/shell.wasm")`. `wasm_task`/`run_at` restano (marcati
  `#[allow(dead_code)]`) per i blob demo `/root/{server,client}.wasm`.

## Perché
Segnalazione utente: "se faccio un exit sull'SSH mi fa exit di tutto
anche sul server". Diagnosi (tastiera→pts/0, SSH→pts/1 sono isolati;
repro headless conferma che uscire dall'SSH **non** tocca la boot
shell): il messaggio `ruos: bin/shell.wasm exited cleanly` viene solo
dalla boot shell, che esce quando si digita `exit` sulla **console
locale** (finestra VirtualBox). Prima di questo fix, una volta uscita,
la boot shell non rinasceva → console locale morta fino al reboot — il
"exit di tutto sul server" percepito.

Comportamento corretto, come `init`/getty su Unix: non puoi davvero
"sloggarti" dall'unica console fisica, semplicemente ricompare. Le
sessioni SSH restano indipendenti (dispatcher su pts/1..3) e non sono
toccate. Si appoggia su [[166-26-05-30-pty-watchdog]] (gestione
lifecycle PTY) e [[164-26-05-30-exec-inherit-pty]] (`--no-init`).

## Test
- `make run-test` → TEST_PASS
- `make run-ssh-test` → TEST_PASS_SSH (la boot shell sopravvive alla
  sessione SSH, confermato)
- Repro respawn (init temporaneo con `exit`): serial mostra
  `boot shell exited code=0 — respawning` seguito da un nuovo
  `ruos shell ready` senza replay dell'init. Verificato e poi rimosso.

## Nota / follow-up
Le code `EXEC_QUEUE` e la pipeline sono ancora single-slot condivise tra
boot shell e shell SSH: esecuzioni concorrenti possono accavallarsi
(hang / exit-code incrociati), anche se non causano l'uscita della
shell. Refactor a multi-slot = TODO separato.

## File toccati
- kernel/src/wasm/mod.rs
- kernel/src/executor/mod.rs
