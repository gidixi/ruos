# 00 — Spec design gestore memoria

**Data:** 2026-05-27

## Cosa

- Definita la roadmap dei 5 sotto-progetti per evolvere MinimalOS in un OS che
  parte su PC reali x86-64 via USB.
- Scritta la spec di design del sotto-progetto #1 (gestore memoria) in
  `docs/superpowers/specs/2026-05-27-memory-manager-design.md`: frame allocator
  a bitmap inizializzato da E820, API paging a pagine 4 KiB, heap kernel buddy.
- Creato `CLAUDE.md` con le regole del progetto (changelog, git, spec, stile).
- Creata la convenzione changelog `CHANGELOG/NN-yy-mm-dd-slug.md`.

## Perché

Avvio del progetto di evoluzione dell'OS. Il gestore memoria è la fondamenta:
multitasking e filesystem ne dipendono. Tracciamento delle modifiche tramite
changelog richiesto dall'utente.

## File toccati

- docs/superpowers/specs/2026-05-27-memory-manager-design.md
- CLAUDE.md
- CHANGELOG/00-26-05-27-memory-manager-spec.md
