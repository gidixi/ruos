# 195 — cpu/ap.rs: AP worker loop (Fase 2)

**Data:** 2026-06-01

## Cosa

Sostituito il loop idle (`hlt`) di `ap_entry` con la chiamata a `ap_worker_loop()`.
La nuova funzione:

- Risolve `cpu_id()` una volta sola prima del loop (stabile per il core).
- Chiama `crate::smp::pool::take()` in loop; se restituisce `Some(slot)`, invoca
  `crate::smp::pool::run_slot(slot, me)` per eseguire il job sul core corrente.
- Se la coda è vuota (`None`), esegue `core::hint::spin_loop()` (istruzione PAUSE)
  invece di andare in halt — gli AP busy-pollano il job queue senza STI/IPI.

Aggiornato anche il doc comment del modulo (`//!`) per riflettere Fase 2 (worker
loop) al posto di Fase 1 (parking idle).

## Perché

Task 2 SMP Fase 2: gli AP devono prelevare ed eseguire job pure-CPU dal pool
(Task 1) invece di stare fermi. Il busy-spin è intenzionale in Fase 2 (nessuna
infrastruttura IPI/interrupt sugli AP); le prestazioni non sono il focus ora,
la correttezza sì. `cpu_id()` viene catturato una sola volta per core e passato
a `run_slot` per registrare quale core ha eseguito il job (utile per verificare
il parallelismo nei test successivi).

## File toccati

- kernel/src/cpu/ap.rs
- CHANGELOG/195-26-06-01-ap-worker-loop.md (questo file)
