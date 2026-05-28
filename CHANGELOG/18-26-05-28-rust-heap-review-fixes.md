# 18 — Review fix milestone Step 4 (heap)

**Data:** 2026-05-28

## Cosa

Correzioni dalle code-review del milestone heap:
- `kernel/src/memory.rs`:
  - Aggiunto commento `// SAFETY:` esplicito sulla `unsafe { ALLOCATOR.lock().claim(...) }`
    che enumera HHDM, USABLE filter, no-aliasing, `'static` ALLOCATOR.
  - Commento sul filtro `MEMMAP_USABLE` marcato "load-bearing" per memoria.
  - Pulizia: `use` al top del modulo (no più sparsi); rimosso commento orfano;
    `Display` di `ClaimFailed` ora `"claim failed"` (consistente con gli altri).
  - **Persistenza `HeapInfo` via `spin::Once<HeapInfo>`** + accessor pubblico
    `heap_region()` → Step 6 (frame allocator fisico) può mascherare il range
    heap-claimato ed evitare di servirlo come frame libero.

## Perché

Chiudere i rilievi delle review prima del merge: rendere esplicito il safety
contract dell'unica unsafe del milestone e impedire la corruzione silenziosa
quando Step 6 leggerà la stessa memory map.

## File toccati

- kernel/src/memory.rs
- CHANGELOG/18-26-05-28-rust-heap-review-fixes.md
