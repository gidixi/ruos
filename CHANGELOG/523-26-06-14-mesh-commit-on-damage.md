# 523 — mesh-path commit-on-damage: skip ship se la mesh non cambia (menu aperto)

**Data:** 2026-06-14

## Cosa
Diagnosi (osservazione utente: menu app aperto + mouse FERMO → scatta in continuo):
con le animazioni off (522) e il mouse fermo la mesh tessellata è IDENTICA ogni
frame, e il kernel salta già il raster (`plan_damage`→None). Il costo residuo è la
shell full-screen che egui ridisegna in CONTINUO col menu aperto: ogni tick
ri-esegue `ctx.run` + tessellate + **`ship_mesh`** (che spediva la mesh
INCONDIZIONATAMENTE) + il kernel re-decodifica + re-hasha tutte le primitive.

Il pixel-path aveva il **commit-on-damage** (`if dirty.w==0 {return}`); il mesh-path
l'aveva PERSO nella migrazione. Ripristinato: `ship_mesh` confronta la mesh
(verts/idx/prims byte-per-byte + w,h) con l'ultima spedita; se IDENTICA e senza nuove
texture → **NON ri-spedisce** (niente `wm.commit_mesh`) → il kernel tiene la surface
in cache (zero decode/plan_damage/raster/present per quella finestra). Confronto
guest-side O(bytes), comunque < ship (host-call) + decode + hash kernel → vince sui
frame invariati (il caso del menu aperto in repaint continuo).

## Perché
Era il vero costo del "menu aperto scatta anche fermo": repaint continuo di egui ×
ri-spedizione+ri-decodifica della shell full-screen ad ogni tick, pur con mesh
identica. Saltare la ri-spedizione azzera il lavoro kernel sui frame invariati.
Beneficio generale: ogni app che egui tiene "sveglia" senza cambiamenti reali
(menu/popup aperti, idle) non ricarica più il kernel. Solo ruos-window (on-device);
non tocca il rasterizzatore né la bit-identità.

## File toccati
- ruos-desktop/crates/ruos-window/src/lib.rs (ship_mesh: LAST_VERTS/IDX/PRIMS/WH +
  skip commit se mesh invariata e nessuna nuova texture)
