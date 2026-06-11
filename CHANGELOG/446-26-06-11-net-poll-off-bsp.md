# 446 — net_poll spostato dal BSP a un ComputeApp core

**Data:** 2026-06-11

## Cosa
Il polling di rete (`net::poll()` a 100 Hz) non gira più sul BSP (core 0) ma
viene pinnato sul primo ComputeApp core.

- `kernel/src/executor/mod.rs`:
  - estratto `net_poll_loop()` (async fn pura) da `net_poll_task`.
  - nuovo `net_poll_spawner_task` (gira sul BSP): se esiste un ComputeApp core
    (`cpu::first_compute_app_core()`) fa `spawn_on(core, net_poll_task())` con
    retry finché l'AP ha pubblicato il suo executor, poi esce; altrimenti
    (≤2-core) esegue il poll inline sul BSP (comportamento vecchio).
  - `run_core` (BSP) ora spawna `net_poll_spawner_task` al posto di
    `net_poll_task`.
  - dà uno scopo a `cpu::first_compute_app_core()` (prima dead-code).

## Perché
Il set I/O del BSP girava tutto su core 0. Sotto traffico sostenuto `net::poll()`
a 100 Hz consuma cicli reali sul BSP, che è l'hub I/O (net/usb/ssh/shell).
Spostandolo su un AP si libera il BSP.

Sicuro da spostare, verificato:
- `net_poll_task` è `Send` (nessuno stato wasmi).
- `NET` è `spin::Mutex` + `without_interrupts`, **già** accesso cross-core: le
  socket op (`recv`/`send`) girano da fiber wasm sui compute core. Spostare il
  poller non aggiunge una classe nuova di contesa.
- i driver NIC sono **pure-polling**: nessun ISR RX da co-locare col poll
  (nessun handler IRQ in `kernel/src/net/`).

Lasciato `usb_poll` sul BSP di proposito (latenza input + xHCI ha IRQ).

## File toccati
- kernel/src/executor/mod.rs
- CHANGELOG/446-26-06-11-net-poll-off-bsp.md
