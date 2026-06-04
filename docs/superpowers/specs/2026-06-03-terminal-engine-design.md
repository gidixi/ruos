# Terminal Engine — Design (Round 1)

**Data:** 2026-06-03
**Stato:** approvato (brainstorming) — pronto per writing-plans
**Scope round:** motore terminale lato kernel (veloce + moderno). La UX shell
userspace è un round successivo con spec propria (vedi § Decomposizione).

## Contesto

ruos ha già: console framebuffer (`kernel/src/console/fb.rs`, 337 righe) con
parser ANSI `vte`, font Noto Sans Mono 24px anti-aliased, PTY cooked/raw +
termios + Ctrl-C, e una toolchain `ratatui → wasm32-wasip1` funzionante (app
`rtop` che rende via `AnsiBackend` emettendo ANSI su PTY). Runtime `wasmi`.

Gap che impediscono un terminale "veloce e moderno":

- **Performance:** nessun back-buffer, nessun dirty-rect/damage tracking. Ogni
  glifo è una `write_volatile` diretta su MMIO con blend anti-alias per-pixel
  ricalcolato ad ogni draw. Scroll = `memmove` dell'intero framebuffer + clear
  pixel-per-pixel. Redraw full-screen → flicker e lentezza (brutale su HW reale
  se il FB non è write-combining).
- **Fedeltà:** SGR (`kernel/src/console/ansi.rs`) supporta solo colori 16 + 256.
  Niente truecolor (`38;2;r;g;b`), niente attributi testo (bold/dim/underline/
  reverse). ratatui usa bold + reverse pesantemente → oggi ignorati. Niente
  alternate screen, scroll regions, stili cursore, scrollback.

## Obiettivi (Round 1)

MUST:

1. Back-buffer ibrido + glyph cache + blit dirty (elimina flicker, redraw veloci).
2. Framebuffer mappato **write-combining** (verifica Limine; PAT se serve).
3. Truecolor SGR `38;2`/`48;2`.
4. Attributi testo: **bold, dim, underline, reverse**.
5. Alternate screen `?1049h/l`.
6. Glifi box-drawing `U+2500–U+257F` resi correttamente (bordi ratatui).

SHOULD:

7. Stili cursore `DECSCUSR` (block/underline/bar + blink) + show/hide `?25h/l`.
8. Scroll regions `DECSTBM` (`CSI t;b r`).
9. Scrollback buffer (default 2000 righe) navigabile Shift+PgUp/PgDn.
10. Bracketed paste `?2004h/l`.

## Non-obiettivi (Round 1)

- **Mouse reporting** (`?1000`/`?1006`): dipende dal driver mouse PS/2 (non
  implementato, step 13 roadmap). Trattato in uno step a sé.
- Italic, CJK/wide-char double-width, protocollo tastiera Kitty, OSC 8/52
  (hyperlink/clipboard), Sixel/grafica raster. Fuori scope o nicchia.
- UX shell (autosuggest, syntax highlight, prompt ricco): Round 2, spec propria.

## Architettura

Refactor di `fb.rs` in unità isolate sotto `kernel/src/console/`. Confine chiave:
`grid` non conosce i pixel; `surface` non conosce le celle; `render` è l'unico
ponte tra i due. Ogni unità testabile in isolamento.

| File | Responsabilità | Dipende da |
|---|---|---|
| `surface.rs` | Pixel back-buffer (RAM) + front buffer (FB MMIO) + blit per span dirty + setup write-combining. Solo pixel. | Limine FB info |
| `grid.rs` | Griglia celle (char/fg/bg/attr) + cursore + dirty per-cella + scroll + scrollback ring + alt-screen (2 griglie) | tipi `ansi` |
| `glyphcache.rs` | Cache **maschera alpha** per `(char, weight)`. Miss → rasterizza Noto; box-drawing → procedurale | crate noto |
| `render.rs` | Componi celle dirty → back-buffer (mask × fg over bg), coalesce span, invoca blit | surface, grid, glyphcache |
| `ansi.rs` | SGR esteso: truecolor + struct `CellAttr` | — |
| `vt.rs` | impl `vte::Perform`: aggiorna grid da escape (CUP/SGR/alt-screen/DECSTBM/DECSCUSR/paste) | grid |
| `mod.rs` | trait `Console`, `FramebufferConsole` wiring, flush policy | tutti |

## Modello dati

```rust
struct Cell { ch: char, fg: Rgb, bg: Rgb, attr: CellAttr }

bitflags! struct CellAttr: u8 { BOLD; DIM; UNDERLINE; REVERSE }  // italic fuori scope

enum CursorStyle { Block, Underline, Bar }   // + flag blink

struct Grid {
    cells: Vec<Cell>,
    cols: u16, rows: u16,
    dirty_rows: Vec<(u16, u16)>,   // span (min_col, max_col) dirty per riga; vuoto = pulita
    cursor: (u16, u16),
    cursor_style: CursorStyle, cursor_blink: bool, cursor_visible: bool,
    scroll_region: (u16, u16),     // DECSTBM, default (0, rows-1)
}
```

- **Alt-screen:** due `Grid` (primary + alt). `?1049h` salva lo stato primary e
  passa ad alt (pulita); `?1049l` ripristina primary. Scrollback solo su primary.
- **Scrollback:** ring di righe-cella, default **2000 righe** (costante
  configurabile). Shift+PgUp/PgDn muove la finestra di vista; qualunque
  output/input nuovo riporta la vista al fondo. La navigazione scrollback è una
  view read-only: non muta la griglia attiva.

## Glyph cache (decisione chiave)

Cache **NON** indicizzata per `(char, fg, bg)`: col truecolor sarebbero ~16M
combinazioni → esplode. Si cache la **maschera di copertura alpha** per
`(char, weight)` — poche centinaia di entry. Al blit:
`pixel = blend(bg, fg, mask_alpha[x,y])`.

Derivazione attributi al momento della composizione (nessuna entry extra in cache):

- **bold** → variante font Bold del crate Noto se disponibile, altrimenti
  embolden sintetico (doppio strike orizzontale di 1px).
- **reverse** → swap fg/bg prima del blend.
- **dim** → fg scalato verso bg (es. 60%).
- **underline** → linea orizzontale disegnata sull'ultima riga della cella.
- **box-drawing `U+2500–U+257F`** → se il crate Noto non copre il range, render
  **procedurale**: la maschera è calcolata da segmenti (linee/giunzioni) in base
  al codepoint. Garantisce bordi ratatui netti indipendentemente dal font.

La maschera è rasterizzata una sola volta per char; i frame successivi sono
memcpy + blend, non blend anti-alias da zero (il collo di bottiglia attuale).

## Pipeline rendering & flush

```
app stdout → PTY → console.write(bytes) → vte parse → Perform aggiorna Grid + marca celle dirty
                                                              │
   flush (coalescato) ◄───────────────────────────────────────┘
   per ogni riga dirty:
       componi celle dirty → back-buffer (glyph mask × fg over bg)
       coalesce in span di pixel contigui
       blit span → front buffer (FB MMIO, write-combining)
   reset dirty
```

**Flush policy** — evita un blit per byte: si accumulano i dirty e si fa flush su:

- **(a)** tick timer coalescante a **~60 Hz** (cap frame-rate), e
- **(b)** subito prima di bloccarsi in attesa di input (`poll_stdin`), così la
  UI è sempre aggiornata quando l'utente vede il prompt.

Nessun flush per singola scrittura. Il cursore (incl. blink) è disegnato in fase
di flush come overlay sulla cella corrente, ripristinabile senza ridisegnare.

## Write-combining

Limine di norma mappa il framebuffer write-combining nelle sue page table. Va
**verificato** sul mapping FB attuale di ruos (HHDM/paging proprio). Se le pagine
FB non sono WC, impostare i bit PAT appropriati sul range FB. Misurare il tempo
di blit (TSC) prima/dopo per confermare il guadagno.

## Estensioni VT (`vt.rs` + `ansi.rs`)

| Escape | Funzione | Tier |
|---|---|---|
| `38;2;r;g;b` / `48;2;r;g;b` | truecolor fg/bg | MUST |
| SGR 1 / 2 / 4 / 7 + reset 22 / 24 / 27 | bold / dim / underline / reverse + reset attributi | MUST |
| `?1049h` / `?1049l` | enter / leave alternate screen | MUST |
| `?2004h` / `?2004l` + wrap incolla in `ESC[200~` … `ESC[201~` | bracketed paste | SHOULD |
| `CSI t;b r` (DECSTBM) | scroll region | SHOULD |
| `CSI n q` (DECSCUSR) | stile cursore block/underline/bar + blink | SHOULD |
| `?25h` / `?25l` | show / hide cursore (verificare se già presente) | SHOULD |

`apply_sgr` cambia firma: oltre a `(fg, bg)` ritorna/aggiorna anche `CellAttr`.
Reset `0` azzera fg/bg ai default e pulisce gli attributi.

## Error handling

- no_std, **zero panic nel path di render**: clamp delle coordinate, escape
  sconosciuti ignorati (comportamento già presente).
- Char senza glifo disponibile → glifo replacement `□`.
- Back-buffer e scrollback allocati a `fb_init` da heap. Fallimento alloc al boot
  → fallback alla modalità write-diretto attuale (degrado, non crash).

## Testing

- **Unit (logica pura)** su `grid`/`ansi`: dato uno stream di escape, asserire lo
  stato celle risultante (fg/bg/attr/posizione cursore/switch alt-screen).
  `#[cfg(test)]` host-side dove la logica è pura (nessun pixel).
- **Smoke in-kernel** dietro flag di boot (estende l'harness `make run-*-test`):
  feed di uno stream noto → dump della griglia come testo su seriale → assert.
- **Perf:** TSC su full-redraw (es. 80×25) e su scroll; stampa ms su seriale;
  assert sotto soglia.
- **Visivo:** QMP `screendump` (già usato per il test USB) → assert frame
  non-vuoto / hash stabile su scena fissa.
- **Regressione:** rtop boota e rende a colori corretti; enter/leave alt-screen;
  scrollback shell intatto dopo l'uscita di una TUI full-screen.

## Memoria

Back-buffer ≈ larghezza×altezza×bpp (~8 MB @1920×1080×4). Scrollback ≈
2000×cols×sizeof(Cell). Compatibile con heap + frame allocator attuali.

## Decomposizione

- **Round 1 (questo spec):** motore terminale kernel.
- **Round 2 (spec a parte):** UX shell userspace — Ctrl-R reverse history search,
  history persistente su `/mnt`, syntax highlight, autosuggest ghost-text,
  menu-completion, prompt ricco. Sfrutta truecolor/attributi/perf del Round 1.
- **Step a parte:** mouse PS/2 driver → poi mouse reporting VT.

## Decisioni risolte (log)

- Buffer = **ibrido C** (griglia celle guida il diff; pixel back-buffer è la
  sorgente del blit). Motivo: testo veloce **+ superficie pronta per rlvgl**
  (north-star GUI), evita riscrittura futura. Alternativa più snella scartata:
  solo cell back-buffer (A), niente superficie pixel per la GUI.
- Glyph cache per **maschera alpha** keyed `(char, weight)`, non per colore.
- Scrollback default **2000 righe**, configurabile.
- Italic **fuori** scope Round 1.
- Mouse **fuori** scope (gated su driver mouse PS/2).
