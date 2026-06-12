# 473 — Spec roadmap multithreading WASM (Fase 1 implementabile)

**Data:** 2026-06-12

## Cosa
Nuova spec `docs/superpowers/specs/2026-06-12-wasm-multithreading-roadmap-design.md`:
roadmap a 3 fasi per il MT nelle app `.cwasm` (obiettivo finale: `std::thread`/
rayon via `wasm32-wasip1-threads`) + spec implementabile della Fase 1
(compositor parallelo: `frame()` delle finestre su core diversi del compute
pool + audit di rientranza di tutte le host fn). Fase 2 = wasm-threads MVP
(SharedMemory + `wasi_thread_spawn`, thread = core dedicato, `atomic.wait` =
park hlt/IPI) in outline; Fase 3 (scheduler preemptive) documentata e NON
costruita. Aggiorna il pivot 2026-05-28 senza rovesciarlo.

## Perché
Decisione dal brainstorming con l'utente (2026-06-12): le app moderne usano il
MT, non va precluso. Sequenza audit-first: prima si blinda il kernel alle
chiamate host concorrenti (con beneficio immediato: un'app lenta non blocca il
desktop), poi si abilitano i thread veri su fondamenta verificate.

## File toccati
- docs/superpowers/specs/2026-06-12-wasm-multithreading-roadmap-design.md (nuova)
- CHANGELOG/473-26-06-12-wasm-mt-roadmap-spec.md
