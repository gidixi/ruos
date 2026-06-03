# 236 — prewarm ASCII glyph cache at console init

**Data:** 2026-06-03

## Cosa

Aggiunto metodo `GlyphCache::prewarm_ascii()` che rasterizza tutti i
caratteri ASCII stampabili (U+0020..=U+007E) nel peso Regular al momento
della costruzione di `FramebufferConsole`. Il metodo è chiamato in
`FramebufferConsole::new`, subito dopo la costruzione dello struct e prima
di `me.clear()`.

## Perché

La refactoring terminal-engine ha spostato la rasterizzazione dei glyph
dietro una `GlyphCache` lazy: `GlyphCache::mask(ch, bold)` fa un
`BTreeMap::insert` + `vec![0u8; w*h]` al primo accesso per ogni carattere.
Il panic handler di kernel (`kernel/src/main.rs` ~141-149) stampa il
messaggio di panic sul framebuffer via `FramebufferConsole::write_str` →
`render::flush` → `GlyphCache::mask`. Se il messaggio contiene un carattere
non ancora in cache, il panic path esegue un'allocazione heap. Questo è
problematico se il panic ha avuto origine nell'allocatore (OOM, heap
corruption, lock held): l'allocazione può fault o deadlock e sopprimere il
messaggio on-screen.

Il vecchio `draw_glyph` pre-refactor era allocation-free: il prewarm
ripristina quella garanzia per l'ASCII, che è il 100 % dei messaggi di
panic del kernel.

Bonus: elimina la latenza di prima rasterizzazione al primo keystroke (ogni
tasto premuto che produce ASCII è ora un cache-hit).

## File toccati

- `kernel/src/console/glyphcache.rs` — aggiunto `prewarm_ascii()`
- `kernel/src/console/fb.rs` — chiamata `me.cache.prewarm_ascii()` in `new()`
