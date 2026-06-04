# Terminal engine (back-buffer pipeline) — followups

Followup emersi durante Tasks 1-8 (console back-buffer refactor) e dal
code review di Task 8. Aperti al merge di `feature/terminal-engine` → `main`.
Nessuno blocca merge; F1 è il più visibile.

## ✅ F1 — Stale XOR cursor on non-dirty cell move — RISOLTO (CHANGELOG 247)

**File:** `kernel/src/console/fb.rs` (`tick_cursor`, `write_str`)
**Severity:** 🟡 cosmetic / visible on real HW → FIXED

### Problema

`tick_cursor` XOR-a i pixel del cursore direttamente sul framebuffer (non nel
back-buffer). Quando il cursore si sposta su una cella che non diventa altrimenti
dirty (es. bare cursor-left `\x1b[D`, `\n` su riga non-finale, `\r`), la vecchia
cella non veniva riscritta dal prossimo `render::flush`. Il XOR rimaneva visibile
fino alla prossima scrittura su quella cella.

### Fix applicato (Plan 3 / Task 3, 2026-06-04)

Opzione 1 implementata nella variante `per-write_str` (più semplice e YAGNI):
`FramebufferConsole` traccia `last_cur: (u16, u16)` — la posizione del cursore
all'ultimo `write_str`. All'inizio di ogni `write_str`, prima di `render::flush`,
viene chiamato `self.grid.mark_cell(last_cur.0, last_cur.1)`, forzando dirty la
cella precedente. Il blit la ridisegna dal back-buffer, eliminando il XOR residuo.
`Grid::mark_cell` clampa silenziosamente i valori fuori range (safe dopo alt-screen
swap). Test T41 verifica il comportamento in `engine_test.rs`.

## F2 — WC mapping non esplicitamente verificata su real HW

**File:** `kernel/src/console/fb_init.rs`
**Severity:** 🟢 doc / nice-to-have

Limine mappa il framebuffer write-combining per default (specificato nel Limine
boot protocol). Su QEMU/VBox il WC è effettivo perché il guest usa l'indirizzo
Limine senza remap. Non abbiamo però un test runtime che legga le MTRR/PAT per
confermare WC su baremetal. Se mai si osservano performance di blit scarse su
hardware reale, aggiungere un check PAT e/o un remap esplicito con
`_PAGE_PAT | _PAGE_WRITE_COMBINING`.

## F3 — Panic path alloca su cache-miss di glyph non-ASCII ✅ MITIGATED

**File:** `kernel/src/console/glyphcache.rs`, `kernel/src/console/fb.rs`
**Severity:** 🟢 mitigated (residual: non-ASCII panic messages)

### Problema (rilevato nella review finale del branch)

Dopo la refactoring terminal-engine la `GlyphCache` era lazy: il primo
accesso a ogni `(char, bold)` eseguiva `BTreeMap::insert + vec![0u8; w*h]`
(allocazione heap). Il panic handler stampa il messaggio via
`write_str` → `render::flush` → `GlyphCache::mask`, quindi un panic su OOM
o con heap lock held poteva allocare nel panic path e sopprimere il
messaggio on-screen. Il serial path era e resta allocation-free.

### Mitigazione (CHANGELOG 236, commit su feature/terminal-engine)

`FramebufferConsole::new` chiama `me.cache.prewarm_ascii()` che rasterizza
tutti i codepoint U+0020..=U+007E (ASCII stampabile) nel peso Regular. I
messaggi di panic del kernel usano esclusivamente ASCII, quindi il render
path è ora **alloc-free per tutti i panic ASCII tipici**.

### Residuo

Un messaggio di panic contenente caratteri non-ASCII (es. Rust format
strings con U+2019 right-quote, caratteri Unicode in variabili di debug)
causerebbe ancora una cache-miss → alloc. Accettabile: i panic message
del kernel Rust sono in pratica sempre ASCII. Per eliminare il residuo
completamente si dovrebbe o (a) usare un font baked-in come array statico
senza alloc (richiede redesign del font loader) o (b) fare fallback a
un glyph di sostituzione se il cache è già "congelato" (flag `in_panic`).
Entrambe le opzioni sono deferred post-Plan-2.

---

## ✅ CLOSED

- **F1** — Stale XOR cursor ghost: risolto in Plan 3 / Task 3 (CHANGELOG 247, 2026-06-04).
