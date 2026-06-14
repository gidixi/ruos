# Design — Damage raster e patch dell'atlante font (fix del lag menu)

**Data:** 2026-06-14
**Stato:** approvato (design), pre-implementazione
**Area:** raster kernel-side (`ruos-raster` + mirror `gui-core` + `ruos-window`)

## Contesto e problema

Sul desktop (compositor kernel-side, finestre = app WASM, UI egui rasterizzata in
software dal kernel) l'interazione col **menu/launcher** è scattosa, mentre desktop
vuoto, finestre e spostamento finestre sono fluidi. La shell è la finestra di sfondo
**full-screen**.

### Root cause (LOCALIZZATA con misure su HW reale — changelog 528)

Build `wm-fps,netconsole`, movimento continuo del mouse sul menu (i7-11800H, RTL8168,
netconsole). Confronto idle vs movimento:

| | idle | movimento (peggiore) |
|---|---|---|
| loop | 100 it/s | **8 it/s** |
| ITER avg | 10 ms | **114 ms** |
| raster avg | 0 µs | **99 ms** |
| frame_all | ~0.1 ms | ~0.5 ms |
| present | 0 | ~10 ms |
| **ra_max** | 0 | **326 ms** |
| **dmg area_max** | 0 | **2.073.600 px = 1920×1080 (FULL SCREEN)** |
| dmg rows / bands | 29 / 1 | **1080 / 15** |

Il collo **non** è `frame_all` (egui resta ~0.5 ms), **non** present (~10 ms), **non**
decode/plan/clone (µs): è **`dispatch_raster` che ri-rasterizza l'INTERO schermo**.
Durante l'interazione `plan_damage` ritorna **FULL damage** → re-raster di 1920×1080
(~300 ms/frame) → il loop crolla a 8 it/s → il lag.

### Perché `plan_damage` ritorna FULL

`plan_damage` (`ruos-raster/src/lib.rs:434`) forza full damage in tre casi:
`realloc`, **`tex_dirty`**, o un hash-diff che copre tutto. Un report di misura
(T+109) ha mostrato danno **piccolo** (area 55.680 = popup) → l'hash-diff funziona e
l'hash del wallpaper è **stabile** → i frame full **non** sono hash-change ma
**`tex_dirty`**.

`tex_dirty` è alzato da `set_texture` (`lib.rs:405`), chiamato via
`wm.tex_update` ← `ship_mesh` **solo** quando `deltas.set` non è vuoto, cioè quando
egui **patcha l'atlante font** (glifi di voci menu/tooltip aggiunti lazy).

### Il vincolo che rende il problema non banale

egui usa **un'unica texture `Managed(0)`** sia per i glifi sia per i fill solidi:
il texel bianco riservato (`WHITE_UV = (0,0)`) vive **dentro** l'atlante font. Il
wallpaper full-screen e tutti i pannelli sono fill solidi con `tex_id = 0`, `uv =
(0,0)`. Quindi un patch dell'atlante font "sporca" la stessa texture che campiona il
wallpaper, e l'attuale `tex_dirty → full` non sa distinguere → ri-rasterizza tutto.

## Fatti verificati sul sorgente (epaint 0.31.1)

Letto da `epaint-0.31.1/` sull'host WSL:

1. **Atlante append-only.** `TextureAtlas::allocate` posiziona i glifi su un cursore
   che avanza (sx→dx, righe verso il basso). I glifi esistenti non vengono **mai**
   riscritti; `allocate_glyph` rasterizza un glifo una sola volta (cache per glyph_id).
2. **Texel bianco (0,0) riservato e immutabile.** Scritto una volta in
   `TextureAtlas::new`, mai più toccato (i glifi crescono altrove; su overflow il
   cursore riparte a riga `height/3`, mai a (0,0)).
3. **Crescita = raddoppio altezza + re-upload completo** (`delta.pos = None`, nuove
   dimensioni). Un append puro entro l'altezza corrente = patch parziale
   (`pos = Some`, sub-rect dei nuovi glifi).
4. **UV normalizzate al tessellate-time** (`uv * (1/atlas_size)`). Una crescita
   d'altezza cambia la uv normalizzata di **tutti** i vertici di testo → i loro
   vertici cambiano → l'hash-diff li intercetta. Ma `WHITE_UV = (0,0)` è una costante
   passata **verbatim** (non moltiplicata per `uv_normalizer`): **(0,0) è
   grow-invariant** → i prim white-only non cambiano mai hash su una crescita.

**Conseguenza:** un prim che campiona **solo** il texel (0,0) ha pixel che **non
cambiano mai** su nessun patch/crescita dell'atlante → si può tranquillamente **non
ri-rasterizzarlo**. Il `tex_dirty → full` attuale è una rete di sicurezza troppo
conservativa: l'unico caso che protegge davvero (pixel cambiano sotto geometria
identica) sull'atlante egui riguarda **solo i glifi**, mai i fill.

## Obiettivi / non-obiettivi

**Obiettivi (SP1):**
- Eliminare il re-raster **full-screen** su patch dell'atlante font, in modo
  **generale** (qualsiasi finestra con sfondo a fill solido, incluse app massimizzate
  future), non solo il menu.
- **Zero pixel stale** (mantenere la garanzia di correttezza).
- **Bit-identità** `ruos-raster` ↔ `gui-core` preservata (cross-check verde).

**Non-obiettivi (rimandati, spec separate):**
- **SP2:** costo raster **per-pixel** su finestre overdraw-heavy (System Monitor
  ~150 ms) — exact-span / rimozione dei 3 `edge()` f64 per pixel.
- **SP3:** present clip-su-damage (present full-screen ~10 ms, minore).

## Design

Due livelli complementari.

### Livello 1 — Core: damage che risparmia i prim white-only (la garanzia)

Modifica **specchiata** in `ruos-raster/src/lib.rs` e
`ruos-desktop/crates/gui-core/src/raster.rs` (coppia bit-identica).

- **`PrimMeta`** (`lib.rs:319` / `raster.rs:72`): aggiungere `white_only: bool`.
- **`prim_meta`** (`lib.rs:613` / `raster.rs:330`): nel loop vertici esistente,
  `white_only &= (u == 0.0 && v == 0.0)` (init `true`). Costo zero (stesso loop).
  Definire `const WHITE_UV: (f32, f32) = (0.0, 0.0)` come discriminatore documentato;
  in `gui-core` riusare `epaint::WHITE_UV`. Un prim senza vertici resta `white_only =
  true` (nessun pixel → mai da danneggiare).
- **`plan_damage`** (`lib.rs:434` / `raster.rs:190`): ristrutturare il branch:
  - `realloc` → `damage = full` (invariato).
  - altrimenti:
    1. hash-diff esistente (added/removed per contenuto) → `damage`;
    2. se `tex_dirty` (`ruos-raster`) / `tex_changed` (`gui-core`):
       `for m in &meta { if !m.white_only { damage = damage.union(m.bbox); } }`.
  - dilatazione +1px e clamp finali invariati; `tex_dirty = false` a fine funzione
    invariato.

Effetto: su patch atlante, il wallpaper full-screen (white-only) è **risparmiato**;
il danno = unione dei bbox dei prim atlas-dependent (testo/immagini), tipicamente
poche decine di righe → `dispatch_raster` prende il path **inline** (`n_bands ≤ 1`),
pochi µs invece di ~300 ms.

**Scope core (YAGNI).** Si parte col solo `white_only`. Risolve il caso riportato
(wallpaper a gradiente) e ogni finestra con sfondo a fill solido. Il raffinamento
**per-tex-id** (risparmiare anche un wallpaper-**immagine** `User`-texture su un patch
del **font**) è documentato come estensione futura (richiede tracciare gli id
patchati come stato da specchiare; senza, al più si ri-rasterizza un'immagine non
cambiata — mai pixel stale). Più piccolo = più sicuro.

### Livello 2 — App: pre-warm dell'atlante (difesa in profondità, rischio zero)

App-side, nessun tocco al core. Riduce ~a zero i patch durante l'interazione.

- **`ruos-window`** (`crates/ruos-window/src/lib.rs`): dopo il primo `ctx.run`
  (gate `static PREWARMED: bool`), `ctx.fonts(|f| f.has_glyphs(&FontId::…, set))`
  per le taglie **non** già pre-caricate da egui: proportional **14.0** (titlebar
  CSD) e monospace **10.0** (System Monitor). Le altre (9.0/12.5/18.0 prop, 12.0
  mono) egui le pre-carica già (`preload_common_characters`).
- **shell** (`apps/shell` + `gui-core/src/desktop/shell.rs`): al cambio del catalog,
  pre-warm dei titoli app a `proportional(12.5)` (i titoli sono UTF-8 dinamici
  forniti dalle app via `manifest()`).

Nota: col Livello 1, anche un patch che sfugge al pre-warm costa poco (danno = solo
testo). Il pre-warm è complementare, non la garanzia. Non aiuta il caso
atlas-recreate all'80% pieno (ma anche lì il Livello 1 limita il danno al testo).

## Correttezza e invarianti

- Texel (0,0) bianco riservato, scritto una volta, mai ri-scritto → un prim
  white-only **non diventa mai stale** su un patch → safe non ri-rasterizzarlo.
- La rete di sicurezza resta per i **non**-white-only: ogni prim che campiona
  contenuto reale dell'atlante è ancora ri-rasterizzato su patch → niente testo
  stale. Su atlas-grow il wallpaper resta `uv=(0,0)` (risparmiato) e il testo
  (uv cambiate) è ridipinto via hash-diff.
- `raster_band` (i **pixel**) non si tocca: cambia solo il **rettangolo** di danno,
  non i valori dei pixel.

## Bit-identità

`ruos-raster::raster_band`/`plan_damage`/`prim_meta` e il mirror
`gui-core::raster.rs` devono restare identici nella logica di damage-planning. Poiché
i pixel non cambiano, il crosscheck single-frame esistente
(`ruos_raster_matches_gui_core_bit_identical`) resta verde per costruzione. Il rischio
è una **divergenza della logica di damage** tra i due impl: vanno specchiati in
lockstep `white_only` (init + AND-fold) e il branch tex-dirty. Si **estende** il
contratto con un nuovo cross-check del patch-damage (vedi Test).

## Test

- Riscrivere `ruos-raster/src/tests.rs::texture_patch_forces_full_damage` →
  `texture_patch_damages_only_atlas_dependent_prims`: scena wallpaper white-only + un
  prim "glifo" con `uv ≠ (0,0)`; patch atlante → il danno copre **solo** il glifo,
  non il wallpaper; e canvas incrementale **==** full render (oracolo no-stale).
- **Nuovo** `ruos-raster/tests/crosscheck.rs::tex_patch_damage_matches_gui_core`:
  stessa scena + patch nei due impl → asserire rect di danno **e** byte del canvas
  identici (oggi il crosscheck è solo single-frame).
- Mirror test lato `gui-core` (la sua CLAUDE.md richiede TDD sul raster).
- Invariati: `dirty_rect_*`, `unchanged_scene_yields_empty_dirty`,
  `banded_matches_serial_bit_identical`, `damage_on_prim_count_change_matches_full`,
  `set_clear_transparent_forces_full_and_bg`, wire/codec.
- **Verifica HW finale** (build `wm-fps,netconsole`): ri-misurare il movimento sul
  menu → `area_max`/`ra_max` devono crollare (atteso ~300 ms → pochi ms, loop ≥60
  it/s).

## File toccati (SP1)

- `ruos-raster/src/lib.rs` — `PrimMeta`, `prim_meta`, `plan_damage`, `WHITE_UV`.
- `ruos-raster/src/tests.rs` — riscrittura test patch.
- `ruos-raster/tests/crosscheck.rs` — nuovo cross-check patch-damage.
- `ruos-desktop/crates/gui-core/src/raster.rs` — mirror `PrimMeta`/`prim_meta`/`plan_damage`.
- `ruos-desktop/crates/ruos-window/src/lib.rs` — pre-warm glifi.
- `ruos-desktop/apps/shell/src/lib.rs` + `gui-core/src/desktop/shell.rs` — warm titoli catalog.
- `kernel/src/wasm/wt/wm.rs` — nessuna modifica logica (il damage più stretto fluisce
  attraverso `raster_meshes`/`dispatch_raster` invariati).

## Rischi e domande aperte

- **Divergenza dei due impl** sulla logica di damage (mitigato dal nuovo cross-check).
- **Wallpaper-immagine** (`User`-texture): col solo `white_only` viene ri-rasterizzato
  su patch del font (conservativo, mai stale). Risolto dall'estensione per-tex-id se
  servirà.
- **atlas-recreate all'80%** (`pos=None`): il Livello 1 limita comunque al testo; il
  pre-warm può ridurne la frequenza ma non eliminarlo.
- **ppp = 1.0** assunto (input.rs); se il kernel usasse ppp ≠ 1.0 le taglie di
  pre-warm andrebbero scalate.

## Decomposizione (ombrello "integrità raster kernel-side")

- **SP1 (questo spec):** white-texel damage separation + pre-warm.
- **SP2 (spec separata):** raster per-pixel exact-span per overdraw (System Monitor).
- **SP3 (spec separata, opzionale):** present clip-su-damage.

## Riferimenti

- Misure HW: changelog 528; `netconsole.log` / log sessione.
- Memoria: `kernel-side-raster` (cronologia fix 513–528 + root cause).
- Spec base raster: `docs/superpowers/specs/2026-06-13-ui-kernel-side-raster-design.md`.
