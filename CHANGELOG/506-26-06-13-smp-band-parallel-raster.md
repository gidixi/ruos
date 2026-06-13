# 506 — Raster kernel-side band-parallelo su pool SMP (`dispatch_raster`)

**Data:** 2026-06-13

## Cosa

Il raster kernel-side della mesh (introdotto in 505, `Compositor::raster_meshes`)
era SERIALE: ogni finestra mesh-mode rasterizzava su un solo core con
`raster.render_wire(...)`. Ora il raster di UNA finestra viene splittato in BANDE di
righe disgiunte che girano sul pool di compute SMP (gli AP), esattamente come
`dispatch_bands` parallelizza il compositing dello schermo.

- **Nuova macchina a bande (mirror di `BandArg`/`BAND_ARENA`/`composite_band_job`):**
  - `struct RasterBandArg` (`#[repr(C)] Copy`, POD: puntatori grezzi come `usize`)
    porta base+len del canvas della banda, `width`, `[y0,y1)`, il rect di damage,
    `clear` impacchettato LE, e i puntatori a `verts`/`idx`/`prims`/`textures` con
    le rispettive lunghezze.
  - `static mut RASTER_BAND_ARENA: [RasterBandArg; MAX_BANDS]` — la GUI core riempie
    `[0,n)`, sottomette una view `&'static [u8]` dello slot, e BLOCCA sul join prima
    di riusare/liberare i buffer → nessun job in volo dangle.
  - `RASTER_CORE_MASK: AtomicU32` + `take_raster_core_mask()` — bitset dei core che
    hanno eseguito un job raster (come `COMPOSITE_CORE_MASK`).
  - `raster_band_job(input: &[u8]) -> u64` — `read_unaligned` dell'arg, ricostruisce
    la `Band` + le slice da puntatori grezzi, chiama `ruos_raster::raster_band(...)`,
    registra il core.
- **`dispatch_raster(raster, verts, idx, prims, dmg)`:** splitta le righe di damage
  `[dmg.1, dmg.3)` in `n_bands = min(max(cpus_online,1), MAX_BANDS)` bande disgiunte
  (`band_rows` ceil), riempie l'arena + `pool::submit` un job per banda, fallback
  inline 1-CPU/pool-pieno (drain `take`/`run_slot` + raster diretto delle bande
  rimaste), poi JOIN con work-steal (`poll_done`/`take`/`run_slot`) prima di tornare.
  Bande con righe canvas DISGIUNTE → nessun aliasing (stessa invariante di
  `dispatch_bands`). Il test host `external_band_split_matches_render_wire` di
  `ruos-raster` prova che questo split è BIT-IDENTICO a `render_wire` seriale.
- **`raster_meshes` riscritta:** decodifica `verts`/`idx`/`prims` in locali della
  GUI core (chiude il borrow dei `mesh_*` prima del `&mut raster`), `plan_damage`,
  `dispatch_raster`, poi legge `raster.canvas()` DOPO il join in `pixels`.
- **Marker `raster cores=N`:** one-shot dopo frame 30 (mirror di `composite cores=`),
  greppabile dallo smoke headless per provare il raster multi-core.
- **Timing `wm-fps`:** nuovi accumulatori `ra_sum`/`n_raster` attorno a
  `raster_meshes()`; aggiunto `raster avg={}us` alla riga `wmfps` (mirror di
  `fa_sum`/`pr_sum`).

`ruos-raster`, `compose_window`, il path legacy `wm.commit`, `dispatch_bands`/
`composite_band` e `present` restano INVARIATI: toccato solo `wm.rs`.

## Perché

Spostare il raster pesante della UID fuori dal singolo core: il pool SMP esistente
parallelizza la rasterizzazione della shell full-screen (1280x800) a bande, come già
fa per il compositing. Nessun thread alle app, nessuna regressione visiva (split
bit-identico al seriale provato dal test host).

## Verifica

`make iso CARGO_FEATURES=wm-fps` → compila. Boot headless QEMU (q35, `-cpu max`,
`-smp 4`):

```
ruos boot OK
[T+13.046s] INFO wm   mesh render win=0 1280x800
[T+13.682s] INFO wm   composite cores=4 [0, 1, 2, 3]
[T+13.683s] INFO wm   raster cores=4 [0, 1, 2, 3]
[T+15.601s] INFO wmfps ... raster avg=1204us ...
```

`raster cores=4` (≥2 → raster SMP girato su tutti e 4 i core), `composite cores=4`,
ZERO panic/PANIC, ZERO WATCHDOG.

## File toccati

- kernel/src/wasm/wt/wm.rs
- CHANGELOG/506-26-06-13-smp-band-parallel-raster.md
