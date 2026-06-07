# 295 — Design: SMP shared-nothing / thread-per-core

**Data:** 2026-06-05

## Cosa
Documento di design (north-star, nessun codice) per portare ruos a sfruttare i
4–8+ core in modo **prestante e solido** restando fedele al pivot (sandbox = WASM,
niente preemption/ring 3, single address space):
`docs/superpowers/specs/2026-06-05-smp-shared-nothing-architecture-design.md`.

Contenuto, fondato sul codice attuale (riferimenti file:riga):
- **Modello attuale**: executor cooperativo singolo sul BSP; AP parcheggiati come
  compute-pool puro (`fn(&[u8])->u64`); I/O tutto sul BSP; il compositor blocca
  l'executor (`run_compositor_gate` non ritorna → SSH/rete affamati).
- **Inventario stato condiviso** categorizzato (globale vero / read-mostly / RW-hot /
  già per-core / IRQ-safe) con i riferimenti ai `static`/`IrqMutex`.
- **Architettura target**: un executor cooperativo **per core** + **ownership** dello
  stato (affinity / per-core replica / RCU) + **message bus** inter-core (no lock
  condivisi) + pin dei workload (I/O core / GUI core / WASM-app core) +
  **supervisione** (heartbeat + restart, fuel/deadline) per la solidità senza
  preemption.
- **Piano di migrazione** per leva (allocatore per-core → message bus → executor
  per-core → audit ownership → pin workload → supervisore → SMP-safety globali) +
  quick-win indipendente (rendere `Compositor::run()` cooperativo per ripristinare
  SSH subito).
- **Trade-off onesti**, anti-pattern da evitare (no preemption + lock ovunque),
  coerenza con la tesi (isolamento per ownership = WASM esteso al kernel).
- **§8 — Il vero rischio del modello:** trusted base illimitata senza contenimento
  hardware (un bug = sistemico), il runtime come security kernel, DMA come killer
  silenzioso, l'SMP `unsafe` che peggiora tutto, niente recovery. Leve di
  correttezza + IOMMU per il DMA.
- **§9 — Il modello elegante (WASM come isolamento universale):** microkernel dove
  driver/FS/net/servizi diventano componenti WASM; trusted base ridotta a nucleo
  nativo minuscolo + runtime; capabilities (Component Model/WASI p2) + IOMMU +
  supervisione. Risolve sia la trusted base illimitata sia l'enforcement mancante
  dello shared-nothing. Analogo: Singularity con WASM come IL. Staging + "lo stai
  già facendo a livello app/GUI".

## Perché
Discussione architetturale: il modello cooperativo single-core è giusto per
l'I/O-bound ma un sottosistema CPU-heavy che non cede (la GUI) affama tutto il
resto (SSH cade in GUI). Fissare la direzione (shared-nothing) prima che altri
sottosistemi calcifichino l'assunzione single-executor.

## File toccati
- docs/superpowers/specs/2026-06-05-smp-shared-nothing-architecture-design.md (nuovo)
