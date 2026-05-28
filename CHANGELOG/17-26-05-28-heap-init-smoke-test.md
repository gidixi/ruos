# 17 — Heap init (Limine memmap + HHDM) + smoke test alloc

**Data:** 2026-05-28

## Cosa
- kernel/src/memory.rs: HeapInfo, HeapInitError + Display, init_heap() che legge
  MemmapRequest e HhdmRequest, sceglie il primo entry USABLE >= HEAP_SIZE,
  calcola virt_base = phys_base + hhdm_offset e fa claim su talc.
- kernel/src/main.rs: nuove richieste Limine MEMMAP_REQUEST e HHDM_REQUEST
  (sezione .requests, bracketed dai marker esistenti); kmain inizializza la
  seriale, controlla base revision, chiama init_heap, logga base+size, esegue
  smoke test Box::new(0xCAFEBABE) + Vec::from(0..5) e logga risultati.
- Makefile: HELLO assert aggiornato alla riga "alloc box=0xCAFEBABE
  vec=[0, 1, 2, 3, 4]" (hello rimane prerequisito implicito); aggiunto flag
  `-F` a `grep` per matching letterale (le parentesi `[...]` nella stringa
  attesa erano interpretate come character class regex).

## Perché
Completa lo Step 4 della roadmap: heap kernel funzionante, alloc utilizzabile da
tutti gli strati successivi.

## Adattamenti API limine 0.6.3
- Tipo request: `MemmapRequest` (non `MemoryMapRequest`).
- Modulo `limine::memmap` invece di `limine::memory_map`.
- Accesso risposta: `.response()` (non `.get_response()`).
- HHDM offset: campo pubblico `hhdm.offset` (non metodo `.offset()`).
- Entry type: campo `e.type_` confrontato con costante `MEMMAP_USABLE`
  (non enum `EntryType::USABLE`).
- Talc 4.4.3: `Span::from_base_size(ptr, size)` e `claim(span)` come da spec.

## File toccati
- kernel/src/memory.rs
- kernel/src/main.rs
- Makefile
- CHANGELOG/17-26-05-28-heap-init-smoke-test.md
