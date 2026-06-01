# 188 — Limine MP_REQUEST + idt::load() per gli Application Processor

**Data:** 2026-06-01

## Cosa
Due aggiunte minimali che preparano il terreno per l'avvio degli AP (Task 2+):

- **`MP_REQUEST`** — Aggiunto `limine::request::MpRequest` a `kernel/src/main.rs`
  come static `#[used] #[link_section = ".requests"]`. Flag `0` (niente x2APIC
  forzato). Il bootloader Limine restituirà la struttura MP con la lista degli AP
  già portati in long-mode al punto di ingresso fornito dal kernel. La response
  viene consumata in un task successivo.

- **`idt::load()`** — Aggiunta funzione pubblica `pub fn load()` in
  `kernel/src/idt.rs` subito dopo `init()`. Chiama `IDT.get().expect(...).load()`
  per caricare la IDT condivisa (costruita da `init()` sul BSP) su qualsiasi core
  che la invochi. Prerequisito: `init()` deve essere già stato chiamato sul BSP.

Nessun `mod smp` aggiunto in questo task (il file `smp.rs` non esiste ancora;
sarà introdotto in un task successivo).

## Perché
Task 1 di SMP Fase 1 (AP bring-up). Il `MP_REQUEST` è necessario affinché Limine
possa avviare gli AP e fornire la loro lista al kernel. `idt::load()` è il hook
che ogni AP chiamerà dopo l'inizializzazione per caricare la IDT condivisa prima
di abilitare le interruzioni.

## File toccati
- kernel/src/main.rs (import MpRequest + static MP_REQUEST)
- kernel/src/idt.rs (aggiunta pub fn load())
- CHANGELOG/188-26-06-01-mp-request-idt-load.md (new)
