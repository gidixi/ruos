# 370 — spec: demand paging della linear-memory Wasmtime

**Data:** 2026-06-09

## Cosa

Aggiunto design doc
`docs/superpowers/specs/2026-06-09-wt-linear-mem-demand-paging-design.md`:
rendere la linear memory WASM (e il codice AOT) demand-paged via #PF handler, così
il minimo dichiarato (48 MiB/finestra) costa solo le pagine toccate invece di
committare tutti i frame all'instantiate.

Contenuto: modello di memoria reale (linear-mem = frame allocator, non heap),
causa radice (`wasmtime_mmap_new` committa eager anche il reserve PROT_NONE),
registry dei range WT, commit-on-fault con disciplina IRQ (deadlock MAPPER vs TLB
shootdown), gestione mprotect/munmap lazy, nota EXEC (un solo meccanismo per dati
e codice), self-test, rischi, abbozzo di piano.

## Perché

L'OOM all'apertura della 3ª finestra desktop è esaurimento frame, non heap. Il
bump heap/RAM (#362) è una pezza; il fix strutturale "da OS adulto" è il demand
paging, che la piattaforma può già supportare (frame allocator + paging +
`pf_handler` risolvibile) senza rebuild app né cambio hash AOT. Lo doc precede
piano e implementazione come da regole di progetto (spec → piano → impl).

## File toccati
- docs/superpowers/specs/2026-06-09-wt-linear-mem-demand-paging-design.md
