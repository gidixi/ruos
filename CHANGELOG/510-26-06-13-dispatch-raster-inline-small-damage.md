# 510 — Fix perf: dispatch_raster scala le bande al lavoro (hover non pega più i core)

**Data:** 2026-06-13

## Cosa
Fix primario della regressione perf su HW reale (riportata: hover vicino al menu →
tutti i core a max + mouse lagga; misurato: `rast`=450µs ma 4/6 core al 50%). Root
cause (workflow di analisi + verifica avversariale): `dispatch_raster` splittava OGNI
raster — anche un damage di poche decine di righe (hover-fade egui) — su
`cpus_online()` bande, una `pool::submit` per banda (ognuna un **wake IPI broadcast**)
+ join **busy-spin**. Lavoro reale di µs, overhead enorme → i core che servono
mouse/USB vengono sfilati da `hlt` → mouse lagga. Il damage piccolo non viene
skippato (è il caso "frame cambiato" dall'animazione hover, non "invariato").

Fix: `n_bands` scalato al lavoro — `min(cpus_online, MAX_BANDS, total_rows/64)` — e se
`n_bands <= 1` il raster gira **INLINE sul core GUI** (niente submit, niente IPI,
niente join). Solo damage grande (≥ ~64 righe/core: first-paint, resize, redraw
ampio) fa fan-out SMP. Bit-identico: il path inline è la stessa `raster_band` del path
SMP/leftover.

Applicato SOLO a `dispatch_raster` (NON `dispatch_bands`: il composite non ha un
damage-rect, una soglia lì romperebbe il comp-smp test).

Verificato (boot headless): full-screen first-paint ancora `raster cores=4`, notify
ok, zero panic. La verifica vera (hover non pega) è su HW reale.

## Perché
Il costo del raster deve essere proporzionale al damage, non al numero di core. Per
cambi UI piccoli/frequenti (hover, animazioni leggere) l'SMP è il problema, non la
soluzione: il floor IPI+wake+spin domina.

## File toccati
- kernel/src/wasm/wt/wm.rs (dispatch_raster: soglia inline + n_bands scalato)
