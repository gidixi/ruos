# 58 — Followups tracciati per Step 9

**Data:** 2026-05-28

## Cosa

Creato `docs/followups/step-9.md` con i 9 followup non-blocking emersi
dai per-task review e dal whole-implementation review dello Step 9
(async executor cooperativo):

- F1: comment init-true su `WAKE_PENDING`.
- F2: ordering SeqCst → Relaxed su `WAKE_PENDING`.
- F3: 0xE0 extended-scancode latching (arrow keys, ecc.).
- F4: drop del dead `< 0x80` clamp nel kbd ISR.
- F5: API reader per `DROPPED` (`pub fn dropped() -> u64`).
- F6: documentazione single-consumer su `read_char`.
- F7: policy per `Delay` slots exhausted (panic → graceful).
- F8: sync spec all'as-built (raw::Executor + WAKE_PENDING).
- F9: wake outside lock in `delay::timer_tick`.

## Perché

Mirror del pattern usato per Step 8 (`docs/followups/step-8.md`,
CHANGELOG 52). Mantiene una home tracked per cleanup opportunistico
durante step successivi. Nessuno dei 9 blocca lo Step 10.

## File toccati

- docs/followups/step-9.md (nuovo)
- CHANGELOG/58-26-05-28-step9-followups.md (nuovo)
