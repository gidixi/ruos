# 524 — plan_damage: diff per CONTENUTO (hash) invece che per posizione

**Data:** 2026-06-14

## Cosa
Misura su HW (hover SOLO sull'icona menu): `ra` (raster) = **46-50ms**, `iter` 89ms.
Per un'icona 24×24 il raster non può costare 50ms → il **damage era quasi
full-screen**. Causa: l'highlight hover dell'icona viene inserito a METÀ lista delle
primitive del pannello; il diff di `plan_damage` (posizionale, anche dopo 521) faceva
slittare di posizione tutte le primitive successive — **incluso il wallpaper
full-screen** — marcandole "cambiate" → damage full-screen → re-raster del gradiente
slow-path = ~50ms su un semplice hover.

Fix: diff per **CONTENUTO** (hash) invece che per posizione. Una primitiva il cui
hash è presente in ENTRAMBI i frame è invariata → nessun danno, indipendentemente
dalla sua POSIZIONE nella lista. Danno solo le primitive presenti in una sola lista
(rimosse → area da ridipingere; aggiunte/cambiate → loro area). Così un highlight
inserito a metà danneggia solo la propria bbox; il wallpaper (hash invariato) non si
ri-rasterizza. Sostituisce il diff posizionale+code di 521.

CORRETTO per egui: l'ordine relativo delle primitive è stabile frame-su-frame, e non
esistono primitive translucide DUPLICATE (l'unico caso in cui il diff per-set
sbaglierebbe). Applicato a ruos-raster + mirror gui-core. Tutti i test verdi
(ruos-raster 14 incl. `damage_on_prim_count_change` + crosscheck byte-identico,
gui-core 44): l'update incrementale resta == al render full.

## Perché
Era il vero costo dell'hover icona menu (e di ogni highlight inline nel pannello):
un inserimento a metà lista promuoveva il wallpaper full-screen a "cambiato" → 50ms
di raster slow-path per niente. Il diff per contenuto rende il damage indipendente
dalla posizione → solo ciò che cambia visivamente viene ridisegnato.

## File toccati
- ruos-raster/src/lib.rs (plan_damage: BTreeSet di hash old/new, danno solo
  added/removed)
- ruos-desktop/crates/gui-core/src/raster.rs (mirror)
