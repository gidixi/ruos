# Terminal engine (back-buffer pipeline) — followups

Followup emersi durante Tasks 1-8 (console back-buffer refactor) e dal
code review di Task 8. Aperti al merge di `feature/terminal-engine` → `main`.
Nessuno blocca merge; F1 è il più visibile.

## F1 — Stale XOR cursor on non-dirty cell move

**File:** `kernel/src/console/fb.rs` (`tick_cursor`, `write_str`)
**Severity:** 🟡 cosmetic / visible on real HW

### Problema

`tick_cursor` XOR-a i pixel del cursore direttamente sul framebuffer (non nel
back-buffer). Quando il cursore si sposta su una cella che non diventa altrimenti
dirty (es. bare cursor-left `\x1b[D`, `\n` su riga non-finale, `\r`), la vecchia
cella non viene riscritto dal prossimo `render::flush`. Il XOR rimane visibile
fino alla prossima scrittura su quella cella.

### Impatto

Puramente cosmetic: nessun dato di testo viene perso. Lieve ghosting del cursore
in scenari di movement-only (cursor navigation nella shell, editing con ← →).

### Fix (deferred)

Due opzioni, da scegliere in Plan 3 / DECSCUSR work:
1. **Force-mark dirty on move**: in `Grid::move_left`, `move_right`, `move_up`,
   `move_down`, `goto` — marcare dirty la cella al cursore *prima* dello
   spostamento (la "vecchia" posizione). Semplice, costo: flush blit extra per
   quella cella ad ogni move.
2. **Composite cursor nel back-buffer**: non XOR sul framebuffer live; invece
   `tick_cursor` scrive nel back-buffer e poi presenta (come una mini-flush di 1
   cella). Richiede che `tick_cursor` acquisisca il lock della console — o che la
   Surface esponga un'API "blit cursor cell". Più clean, più invasivo.

### Contesto architetturale

Oggi `tick_cursor` opera **senza lock**, leggendo soli atomics (FB_VIRT,
FB_PITCH, CURSOR_POS). Questo è deliberato (ISR path). Aggiungere un lock
in `tick_cursor` richiede attenzione: il lock della console non può essere
held dall'ISR a meno che non sia un raw spinlock senza possibilità di
preemption (ok su single-core + `without_interrupts`).

## F2 — WC mapping non esplicitamente verificata su real HW

**File:** `kernel/src/console/fb_init.rs`
**Severity:** 🟢 doc / nice-to-have

Limine mappa il framebuffer write-combining per default (specificato nel Limine
boot protocol). Su QEMU/VBox il WC è effettivo perché il guest usa l'indirizzo
Limine senza remap. Non abbiamo però un test runtime che legga le MTRR/PAT per
confermare WC su baremetal. Se mai si osservano performance di blit scarse su
hardware reale, aggiungere un check PAT e/o un remap esplicito con
`_PAGE_PAT | _PAGE_WRITE_COMBINING`.

---

## ✅ CLOSED

*(nessuno ancora)*
