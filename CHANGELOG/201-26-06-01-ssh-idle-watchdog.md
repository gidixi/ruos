# 201 — SSH idle non disconnette più la shell

**Data:** 2026-06-01

## Cosa
Una sessione SSH lasciata idle non viene più chiusa dal `pty_watchdog_task`.
Due bug nel tracking dell'attività della coppia PTY:

1. **`last_activity` non resettato al claim.** `release` resettava il timestamp,
   ma `try_claim` no: una sessione nuova ereditava lo 0 iniziale (o il valore di
   una sessione precedente). Se l'uptime del kernel superava già il limite idle,
   `now - 0 > limite` → il watchdog chiudeva la sessione entro un intervallo di
   check (= "pochi secondi" dopo il connect). Fix: `try_claim` resetta
   `last_activity = now`.

2. **Il watchdog ammazzava sessioni vive ma idle.** Serve a recuperare le coppie
   *leaked* (disconnessione brutale che bypassa la pulizia), ma non distingueva
   una sessione connessa-ma-inattiva da una leaked. Fix: il bridge SSH fa un
   heartbeat (`touch_activity` a ogni iterazione finché il canale è attivo), così
   una sessione CONNESSA non viene mai recuperata per inattività; il watchdog
   (5 min) resta come backstop solo per coppie il cui bridge è morto.

## Perché
Un utente al prompt SSH può restare idle a tempo indefinito senza essere
disconnesso. Il limite idle del watchdog è solo una rete di sicurezza per i leak.

## File toccati
- kernel/src/pty/mod.rs (try_claim resetta last_activity)
- kernel/src/ssh/sunset_io.rs (heartbeat touch_activity nel bridge)
- kernel/src/executor/mod.rs (commento watchdog)
- tests/ssh-idle-test.sh + Makefile run-ssh-idle-test

## Verifica
`make run-ssh-idle-test`: sessione connessa + idle 25s sopravvive, nessun
watchdog fire. Con limite-debug 6s: idle 20s sopravvive (heartbeat). Regressione
verde: run-ctrlc-test, run-rtop-test, run-test (--once smoke).
