# 362 — terminal: advance_view — logica follow "resta fermo + indicatore"

**Data:** 2026-06-09

## Cosa
Aggiunta funzione pura `advance_view(view_offset, prev_sb, sb) -> (usize, bool)` in
`ruos-desktop/crates/gui-core/src/desktop/apps/terminal.rs`, con suite di 4 test
unitari. La funzione aggiorna il `view_offset` dello scrollback quando arriva nuovo
output: se l'utente è scrollato su mantiene la vista ferma (offset += righe nuove,
indicatore ON); se è a fondo resta a 0 (segue il fondo, nessun indicatore). Sempre
clampato a `sb`.

Note: il test `follow_clamps_at_cap` sub-caso 1 aveva un'asserzione errata
(`assert_eq!(off, 1000)` per input `(999, 1000, 1000)` dove 999 non è oltre il cap)
— corretta in `assert_eq!(off, 999)` per coerenza con l'implementazione di
riferimento e la semantica descritta.

## Perché
Preparazione per Task 4: la struct `Terminal` acquisirà `view_offset` e chiamerà
`advance_view` ogni frame per tenere l'output visibile ma la vista ferma se l'utente
ha scrollato su.

## File toccati
- ruos-desktop/crates/gui-core/src/desktop/apps/terminal.rs
