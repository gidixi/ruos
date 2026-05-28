# 52 — Followups tracciati per Step 8

**Data:** 2026-05-28

## Cosa

Creato `docs/followups/step-8.md` con i 5 followup non-blocking emersi
dal whole-implementation review dello Step 8 (merge `e6d8171`):

- F1: pubblicare `FB_VIRT` *dopo* `clear()` (race ISR/clear).
- F2: `apply_sgr` swallow della tail `;2;R;G;B` quando SGR 38/48 non
  segue `;5`.
- F3: `ConsoleFile::write` instradare via `CONSOLE` o `without_interrupts`
  (deadlock landmine pre-esistente, allargato da Step 8).
- F4: spec drift — togliere `FB_PIXEL_BGR` / `CURSOR_SHOWN` dalla lista
  atomics.
- F5: estendere CSI `J` 0/1 e `K` 0/1/2 prima dello Step 11 (shell).

## Perché

I followup vivevano solo nel body del merge commit `e6d8171`. Trasferiti
in un file tracked nel tree (`docs/followups/`) così sono visibili senza
scavare in git log e affrontabili opportunisticamente quando i file
sottostanti vengono toccati. Convenzione: un file per step.

## File toccati

- docs/followups/step-8.md (nuovo)
- CHANGELOG/52-26-05-28-step8-followups.md (nuovo)
