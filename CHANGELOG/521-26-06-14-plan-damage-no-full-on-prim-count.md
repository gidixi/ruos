# 521 — plan_damage: niente full-damage sul cambio di numero primitive (hover menu)

**Data:** 2026-06-14

## Cosa
Causa di "hover sul menu app fa schifo" (mentre tutto il resto è fluido): aprire il
launcher / mostrare un tooltip fa cambiare il NUMERO di primitive egui, e
`plan_damage` forzava **FULL damage** quando `prev.len() != meta.len()`. La shell è
**full-screen** → ogni frame del menu ri-rasterizzava l'INTERA finestra 1280×800
(wallpaper a gradiente = slow-path compreso). Le finestre piccole non soffrivano (il
loro full-damage è piccolo); la shell full-screen sì.

Fix: egui aggiunge le primitive di popup/tooltip/menu su un layer più alto → in CODA
alla lista. Quindi diff posizionale del prefisso comune + danno solo le CODE
(primitive presenti in una sola lista = aperte/chiuse → la loro bbox). Aprire un menu
ora sporca SOLO l'area del popup, non tutta la finestra. Sempre CORRETTO (sovrastima
al più se egui inserisse a metà; mai sottostima → niente pixel stale).

Applicato a `ruos-raster` (kernel) + mirror `gui-core`. Nuovo test
`damage_on_prim_count_change_matches_full` (ruos-raster): aggiunta/rimozione di una
primitiva → damage PARZIALE + update incrementale == render full, pixel-per-pixel.
Verifica host: ruos-raster 14 + `crosscheck` byte-identico verdi, gui-core 44 verdi.

## Perché
Era il vero collo dell'hover menu: full-screen re-raster ripetuto durante
l'animazione del menu. Con damage parziale il menu sporca poche centinaia di px
invece di 1 Mpx → fluido. Beneficio generale: ogni popup/tooltip/menu in QUALSIASI
app non forza più il full-redraw della finestra.

## File toccati
- ruos-raster/src/lib.rs (plan_damage: diff prefisso + code invece di full su
  len-mismatch)
- ruos-raster/src/tests.rs (test damage_on_prim_count_change_matches_full)
- ruos-desktop/crates/gui-core/src/raster.rs (mirror)
