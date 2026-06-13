# 497 — wasm linear-memory OOM: panic chiaro invece di halt silenzioso del core + -m 2048 obbligatorio

**Data:** 2026-06-13

## Cosa

Root cause del "tool `.cwasm` da SSH+desktop si pianta sull'AP" (issue
`cwasm-ssh-desktop-hang`): **NON** era né SSH-specifico né un bug di demand
paging su AP. Era **esaurimento dei frame fisici**.

- Il `rip` faulting decodifica a **`memcpy`** (`copy_forward`) che scrive nella
  linear memory wasm demand-paged di un guest. Pagina non committata →
  `commit_fault` → `allocate_frame()` ritorna None (frame esauriti).
- Il vecchio comportamento: `commit_fault` ritornava false → l'handler #PF
  (`idt.rs`) faceva **`halt()` del SOLO core faulting**. Morte SILENZIOSA: il
  desktop zoppicava su meno core, e qualsiasi tool lanciato su quel core
  rimaneva piantato. Diagnosi pessima.
- **Fix (`wt/demand.rs`)**: il path alloc-exhausted ora **panica** con un
  messaggio diagnostico — `wasm linear-memory OOM: out of physical frames ...
  (frames total/used/free) ... too little RAM for the heap + wasm working set
  (need ~2 GiB)`. Fallimento LOUD e chiaro invece di un core morto in silenzio.

Perché succedeva: **`HEAP_SIZE` è FISSO a 768 MiB** (`memory/heap.rs`). A
T+0.09s, prima che il desktop renderizzi, sono già usati ~948 MiB. Su `-m 1024`
restano ~75 MiB liberi (19173 frame) per TUTTO il working set wasm
demand-paged (egui shell+notify + codice AOT + page table). Il desktop ne tocca
>75 MiB → esaurimento. Su `-m 1536`+ ci sono ≥587 MiB liberi → nessun problema.
Il commento in `heap.rs` lo diceva già: il sistema è progettato per `-m 2048`.

- **`-m 2048` ora imposto e documentato.** Tutti i `tests/*.sh` bumpati da
  `-m 512`/`-m 1024` a `-m 2048` (i target Makefile erano già a 2048). Nota in
  `CLAUDE.md`. I test SSH/desktop sotto-dimensionati prima "passavano" mentre
  dei core morivano in silenzio (i loro asserti girano sul BSP); ora boota
  pulito.

## Indagine (metodo)

Riproduzione: **deterministica 5/5 HEADLESS** (senza SSH) bootando il desktop
di default a `-m 1024`. SSH/desktop sembravano la causa solo perché i test
relativi (rtop-ssh, ssh-shell, wt-stdin) usavano `-m 1024`. Conferma via il
test dell'ipotesi RAM: `-m 1024` → 19173 frame liberi → OOM; `-m 1536` →
150244 → ok; `-m 2048` → 281316 → ok. `rip` decodificato con `rust-addr2line`.

## Verifica

- `-m 1024` + desktop: `KERNEL PANIC: wasm linear-memory OOM ... free=0` —
  chiaro e diagnosticabile (prima: halt silenzioso del core).
- `-m 2048` + desktop: 0 OOM/#PF/PANIC, compositor + shell ready.
- `make run-test` (`-m 2048`): TEST_PASS.

## File toccati

- kernel/src/wasm/wt/demand.rs
- tests/*.sh (-m → 2048)
- CLAUDE.md
- CHANGELOG/497-26-06-13-wasm-oom-panic-vs-silent-halt.md
