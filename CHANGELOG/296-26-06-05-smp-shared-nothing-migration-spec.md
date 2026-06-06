# 296 — Spec macro: migrazione SMP shared-nothing (foundation + 7 step)

**Data:** 2026-06-05

## Cosa
Spec macro azionabile per l'intero programma §4 del north-star doc, come UN'unica spec
di architettura (ogni step avrà poi spec di dettaglio → piano → impl):
`docs/superpowers/specs/2026-06-05-smp-shared-nothing-migration-design.md`.

Fondata su un'indagine del codice (8 sottosistemi in parallelo + critico di
consistenza). Rispetto al north-star doc padre:

- **Corregge 2 premesse:** (1) i lock attuali sono già spinlock SMP veri (`spin 0.9.8`
  CAS + `IrqMutex`, audit `CHANGELOG/186` zero must-fix) → niente "step-0 fai uno
  spinlock"; (2) gli AP sono già vivi multi-core oggi (`bringup`→`ap_worker_loop`
  drena il pool; marker `composite cores={N}`). La migrazione è **riduzione contesa +
  partizione ownership**, non lock-safety.
- **Identifica il vero gap foundational:** dispatch wake/`__pender` cross-core (oggi
  globale), da costruire UNA volta (Step 2/3 congiunti).
- **Ordine di build corretto:** 7(audit/baseline) → 1(alloc per-core) → 2(bus + wake
  cross-core + tabella vettori IPI) → 3(executor per-core + timer AP + TLB shootdown)
  → {4(ownership), 5(pin GUI)} in parallelo → 6(supervisor). Differisce dal §4 letterale
  (7 primo, 5 parallelo a 4).
- **Modello target CORE_ROLES** (ownership distribuita massimale): BSP=I/O+NET,
  GUI-core=compositor, log-core, pty-core(i), compute/app core(i); proc registry e RNG
  per-core. Degrado dinamico per conteggio core.
- **10 invarianti globali** (fedeltà pivot, single address space, ordine lock
  MAPPER→FRAMES, baseline spinlock, single-writer-per-slot, ISR lock-light, no
  missed-wake, clock monotonico globale, fencing publish/consume, hardware BSP-only).
- **6 gap di correttezza promossi a prima classe** (non nel north-star §4): TLB
  shootdown cross-core, wake `__pender` cross-core, tabella vettori IPI, calibrazione
  timer LAPIC su AP, waker Send/Sync cross-core, CORE_ROLES single source of truth.
- **Rischi cross-step** (free cross-core × InboxMsg Box; TLB × MAPPER × WASM per-core;
  supervisor starvation × compositor bloccante; heap imbalance × GUI pinnata; GEN_COUNTER
  globale × timer AP).
- **Decisioni utente baked-in** (conflitti risolti): ownership distribuita (NET pinnato
  su BSP), log-core async (+ fallback sync panic/boot), proc registry per-core, RNG
  per-core.

Per ogni step: stato attuale (file:riga), target, nuove API (firme), touch points,
test/accettazione, rischi.

## Perché
L'utente ha scelto di specare tutti e 7 gli step §4 insieme (quadro completo prima di
implementare) e l'ordine foundational del doc. Servono il quadro corretto (lock reality,
AP già vivi), l'ordine di dipendenza reale, e i gap di correttezza che il north-star non
copriva, prima di aprire i cicli spec→piano→impl dei singoli step.

## File toccati
- docs/superpowers/specs/2026-06-05-smp-shared-nothing-migration-design.md (nuovo)
- CHANGELOG/296-26-06-05-smp-shared-nothing-migration-spec.md (questo)
