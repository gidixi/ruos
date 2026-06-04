# 269 — Desktop egui: rendering a dirty-rect (ridisegna solo la regione cambiata)

**Data:** 2026-06-04

## Cosa
Il renderer ora mantiene un **canvas persistente** (il frame precedente) e ogni
frame aggiorna + blitta **solo il rettangolo cambiato** (damage rect), invece di
ri-rasterizzare e blittare tutti i 1280×800 a ogni frame. È il fix vero per lo
"scatti" in hover/drag.

## Perché
Feedback utente dopo CHANGELOG 268: "molto meglio ma si può migliorare ancora?".
Il costo residuo era che OGNI frame cambiato (hover bottone, drag finestra,
animazione) ri-rasterizzava l'intera scena — incluso il wallpaper gradiente
full-screen statico — e ne blittava 4 MB. Con egui in immediate-mode la scena è
ri-emessa intera ogni frame, quindi serviva un meccanismo di damage esterno.

## Come
- **Diff per-primitiva**: per ogni `ClippedPrimitive` si calcola hash (clip +
  vertici + indici + texture id) e bbox (estensione vertici ∩ clip, in pixel).
  Si confronta con i metadati del frame precedente.
- **Damage** = unione dei bbox `(vecchio ∪ nuovo)` delle primitive il cui hash è
  cambiato (pad 1px ai bordi, clamp allo schermo). Cambio struttura (numero prim
  diverso) o texture → damage = full. Nessun cambiamento → damage vuoto → si
  ritorna senza blittare (rimpiazza lo skip-when-unchanged di 268).
- **Ridisegno**: si pulisce il solo damage e si ri-rasterizzano TUTTE le
  primitive clippate al damage (così occlusioni e riveli restano corretti), nel
  canvas persistente. Fuori dal damage il canvas trattiene il frame precedente.
- **Present parziale**: si impacchetta il sotto-rettangolo damage (stride canvas
  ≠ larghezza rect) in righe contigue e si chiama `gfx_blit(x,y,w,h)` solo su
  quello. Il blit fast-path del kernel (CHANGELOG 267) gestisce già x,y,w,h
  arbitrari + ricompone il cursore.

Effetto chiave: il **wallpaper full-screen è una primitiva a sé che non cambia
mai** → esce dal damage dopo il primo frame e non viene più ri-rasterizzato.
Hover/drag toccano solo il pannello / la finestra interessata.

## Verifica
- 3 test nuovi in `raster.rs`, deterministici:
  - `dirty_rect_move_matches_full_render`: spostare un rettangolo → canvas
    parziale == render full pixel-per-pixel (prova: nessuna scia / regione stale).
  - `dirty_rect_recolor_matches_full_render`: cambio colore (hover) idem.
  - `unchanged_scene_yields_empty_dirty`: scena invariata → damage vuoto.
- `cargo test -p gui-core` → 10/10 ok. `gui.cwasm` + `os.iso` ribuildati.
- Il fast-path raster uv-costante (268) resta attivo per i frame che SI disegnano.

## File toccati
- ruos-desktop/gui-core/src/raster.rs (canvas persistente + diff/damage + fill_rect + prim_meta + IRect)
- ruos-desktop/gui-core/src/lib.rs (present del solo damage rect + crop_rgba; rimosso lo scene_hash di 268, superato)
