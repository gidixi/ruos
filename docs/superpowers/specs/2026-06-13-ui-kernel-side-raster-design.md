# Design — Raster UI kernel-side (display-server), GPU-less

**Data:** 2026-06-13
**Stato:** APPROVATO (architettura confermata da Giuseppe; target perf assoluti da HW reale)
**Sostituisce:** `2026-06-13-ui-parallel-raster-design.md` (approccio in-wasm A/D) per la
parte di parallelizzazione. Il refactor `gui-core` band-able fatto sotto quella spec
**resta valido** come (a) riferimento bit-identico per il port kernel, (b) raster
dell'anteprima PC. Lo spike "join in frame()" non serve più (niente thread per-app).

## 1. Obiettivo

Rendere la rasterizzazione UI **parallela su più core**, senza GPU, **senza dare
thread alle app** e **senza regressioni** di output o prestazioni. Architettura
"display server": l'app tessella (egui → triangoli) e **spedisce le mesh** al kernel;
il **kernel rasterizza** nella surface della finestra usando il **pool SMP già
esistente** (lo stesso del compositing). Il raster pesante esce da `frame()`.

### Non-obiettivi

- Niente driver GPU. Niente `wasm32-wasip1-threads` per le app finestra.
- Niente cambi al compositor a valle (`composite_band`/present restano identici).
- Niente SIMD manuale del raster (eventuale, dopo, separato).
- Niente nuovo damage-tracking algoritmico: si **porta** quello di `gui-core`
  (`plan_damage` + hash/bbox per-primitiva) nel kernel.

## 2. Contesto / prior art (NON rifare)

Profiling `wm-fps` (TCG, solo rapporti) + lettura codice:

- Compositor kernel-side già "alla Ubuntu": retained per-window surfaces, double
  buffer (`backbuf`), present diff-per-riga (`gfx::blit`), idle-skip, **compositing
  SMP a bande** (`wt/compose.rs::composite_band`, `dispatch_bands`), dormienza
  finestre (`compute_awake`/`should_wake`/`wm.stay_awake`).
- Damage-driven raster già in `gui-core` (`raster.rs::Renderer::render` → canvas
  persistente + diff). Refactor band-able + test equivalenza già fatti
  (commit `ee39f15`/`184a0d9`, branch `feat/ui-parallel-raster`).
- Le app finestra sono `wasm32-wasip1` (single-thread): per questo il raster
  parallelo NON può vivere dentro l'app senza convertirle tutte ai thread (scartato).

Il collo è la **rasterizzazione per-finestra single-thread**. C la sposta nel kernel
e la parallelizza sul pool SMP.

## 3. Flusso per frame (orchestrato dal core GUI, pattern esistente)

```
STADIO 1  per ogni finestra sveglia:  run frame() sul pool
            → drain input → ctx.run → TESSELLATE → wm.commit_mesh(...)   [leggero]
            il kernel COPIA mesh+delta-texture in buffer kernel per-finestra
STADIO 2  per ogni finestra con mesh nuova:  plan_damage (diff) → se damage ≠ ∅:
            dispatch_raster: split del damage in bande disgiunte sul pool SMP
            → ogni banda rasterizza la mesh nella surface kernel della finestra
STADIO 3  composite (dispatch_bands, INVARIATO) → present (gfx::blit, INVARIATO)
```

Tre stadi sequenziali, ognuno usa il pool SMP; il core GUI orchestra e joina con
work-steal — identico a `dispatch_frames`/`dispatch_bands` di oggi. Con 1 core tutto
degrada a seriale = comportamento attuale.

## 4. Componenti

### 4.1 App / `ruos-window` (cambio minimo; app restano `wasip1`)

`frame_once`/`frame_once_bare`: invece di `Renderer::render` (tiny-skia) + `wm.commit`
(pixel), convertono `out.shapes` tessellati + `out.textures_delta` nel **wire format**
(§5) e chiamano `wm.tex_update(...)` (sui delta) + `wm.commit_mesh(...)`. Le crate
`apps/*` NON cambiano (toccano solo `ruos-window`). tiny-skia **esce** dai `.cwasm`
on-device (app più piccole). `gui-core::raster` resta per l'anteprima PC e come
riferimento.

### 4.2 ABI mesh (nuove host fn nel linker `wm`, `kernel/src/wasm/wt/wm.rs`)

- `wm.tex_update(id: u64, x,y,w,h: u32, ptr,len: u32) -> i32` — aggiorna/crea
  l'atlante `id` con un patch RGBA premoltiplicato `w×h` a `(x,y)` (semantica di
  `apply_textures`/`set_texture` di `gui-core`). Chiamata SOLO sui `TexturesDelta`
  (raro: font atlas all'avvio, crescita atlante). Il kernel COPIA i pixel.
- `wm.commit_mesh(verts_ptr,verts_len, idx_ptr,idx_len, prims_ptr,prims_len, w,h: u32) -> i32`
  — la mesh del frame. Il kernel COPIA i tre buffer in memoria kernel per-finestra
  (mai letti dagli AP dalla linear-memory guest → risolve l'accesso multicore + si
  allinea al vincolo single-accessor/multi-tenant). Marca la finestra "mesh-nuova".

Entrambe leggono la guest memory via `wt::mem::read` (accessor auditato, sul core GUI,
sincrono dentro `frame()`), copiano, ritornano. Versionate; documentate in
`docs/api/wm.md`.

### 4.3 Rasterizzatore kernel — crate no_std condivisa `ruos-raster`

**Crate workspace nuova `ruos-raster` (`no_std`, zero dipendenze egui/tiny-skia)**,
così è (a) usabile dal kernel e (b) **testabile su host** per il cross-check. Contiene
il port 1:1 di `gui-core/src/raster.rs`:

- Strutture **wire** (plain, §5): `Vertex { x,y,u,v: f32, color: u32 }`,
  `Prim { clip: [f32;4], tex_id: u64, idx0, idx1: u32 }`.
- `Band` (vista righe), `fill_rect`, `raster_tri`, `raster_band`, `plan_damage`,
  store atlanti (`BTreeMap<u64, Atlas>` dove `Atlas { w,h: u32, px: Vec<u8> }`).
- Buffer pixel = `&mut [u8]` RGBA premoltiplicato (no `tiny_skia::Pixmap`;
  `PremultipliedColorU8` → funzioni inline su `[u8;4]`).
- **`edge` in f64** identica (robusta alla cancellazione f32; il raster kernel è
  Rust NATIVO, non wasm → niente bug libcall `floor/ceil` dell'egui-text-garble).

Il kernel dipende da `ruos-raster`. `gui-core` resta separata (egui+tiny-skia) per il
PC; `ruos-raster` è la versione on-device.

### 4.4 Stato per-finestra (kernel `wm.rs`, struct `Window`)

Aggiungere: `surface: Vec<u8>` (RGBA, kernel-owned, la surface che il compositor
legge), `prev_meta: Vec<PrimMeta>` (per il diff), e i buffer mesh copiati
(`verts/idx/prims: Vec<u8>`) + ref allo store atlanti (per-finestra o condiviso). Le
finestre `commit_mesh` usano questa surface; le finestre legacy `wm.commit` (pixel)
restano sul percorso attuale (vedi §6).

### 4.5 Dispatch raster SMP (`wm.rs`)

`dispatch_raster(window)`: calcola `plan_damage` (sul core GUI, cheap), poi splitta il
damage in N bande di righe disgiunte e le sottomette al pool (`smp::pool::submit`),
join con work-steal — clone di `dispatch_bands`. Ogni banda chiama
`ruos_raster::raster_band` sui buffer mesh kernel + surface kernel. Bande disgiunte →
nessuna sincronizzazione. 1 core → inline seriale.

### 4.6 Compositor: INVARIATO

`compose_window` rende la `surface` kernel della finestra; `composite_band` + present
identici. Il cambiamento è SOLO la **produzione** della surface (kernel raster invece
di app-commit-pixel).

## 5. Wire format (contratto app↔kernel)

Little-endian. Specchia `egui::epaint`:

- **Vertex** (20 B): `pos.x f32, pos.y f32, uv.x f32, uv.y f32, color u32` (color =
  `Color32` premoltiplicato, byte `[r,g,b,a]`).
- **Index**: `u32`.
- **Prim** (28 B): `clip_min_x, clip_min_y, clip_max_x, clip_max_y f32` (4×4),
  `tex_id u64` (Managed→id, User→id|0x8000_0000_0000_0000), `idx0 u32, idx1 u32`
  (range semiaperto in `indices`). Solo mesh; le `Primitive::Callback` (GPU custom)
  si ignorano come oggi.
- **tex_update**: pixel RGBA8888 premoltiplicati, row-major `w×h`.

`ruos-window` produce questo da `ctx.tessellate(...)`; `ruos-raster` lo consuma. Il
formato è identico in numerico ai campi egui → output bit-identico a `gui-core`.

## 6. Regression-safety (requisito esplicito: niente regressioni / cali)

1. **Path doppio durante la migrazione.** `wm.commit` (pixel) resta funzionante;
   `wm.commit_mesh` è il nuovo. Una finestra dichiara quale usa (probe: se chiama
   `commit_mesh` è "mesh-mode", altrimenti legacy pixel). Le app migrano **una alla
   volta**. Nessun big-bang; le non-convertite girano identiche.
2. **Guardia bit-identica (doppia).**
   - `gui-core`: test `banded_matches_serial_bit_identical` (già verde).
   - **Cross-check host nuovo**: stessa scena egui → (a) `gui-core::raster` (tiny-skia)
     e (b) `ruos_raster` (via wire format) → `assert_eq!` byte-per-byte sulla surface.
     Vive in `ruos-raster` o in un test host che dipende da entrambe. È il guardiano
     del port: una divergenza di un pixel fa fallire la CI.
3. **A/B per app.** Converto `terminal`, confronto **visivo** + `wm-fps` prima/dopo
   su HW reale. Avanti solo se identico e non più lento.
4. **Perf non cala per costruzione.** Stesso lavoro di raster, spostato sul kernel e
   parallelizzato. Caso 1-core = seriale = come oggi. Mesh ≪ surface → trasferimento
   wasm→kernel **cala** (oggi `commit` manda `w·h·4` byte/ frame; la mesh è ~KB).
   Atlante grande solo sui delta (raro).

## 7. Rischi + mitigazioni

| Rischio | Mitigazione |
|---|---|
| Divergenza pixel kernel↔gui-core | cross-check host bit-identico (golden = gui-core); f64 edge portata 1:1 |
| Accesso guest-mem dagli AP | mesh/atlante COPIATI in kernel-mem al commit; gli AP leggono solo kernel |
| Determinismo f32 | raster kernel = Rust nativo (no libcall wasm); edge f64; cross-check |
| Atlante font grande | solo su `TexturesDelta`; copiato una volta |
| Nesting pool (frame/raster/composite) | stadi sequenziali; join work-steal esistente; 1-core seriale |
| Memoria surface per-finestra | una `Vec<u8>` RGBA per finestra (come la surface committata oggi) |
| ABI nuova | versionata, `docs/api/wm.md`; path pixel resta finché stabile |

## 8. Misura / criteri di successo

- Strumento: `wm-fps` (esiste). Misura vera su HW reale (i7-11800H), workload sotto
  carico (terminal con molto testo / scroll lista / egui demo).
- **Baseline (TODO HW):** `frame_all avg` (ora include raster app-side), `present avg`,
  `fps`. Dopo C, `frame_all` = solo tessellation (deve CALARE); il raster appare come
  nuovo stadio kernel (da strumentare con un contatore `raster avg` dietro `wm-fps`).
- **Target:** raster kernel parallelo ≥ 2.5× sul caso finestra grande a 4 core;
  `frame_all` (tessellation) molto più basso di prima; **zero** regressioni pixel
  (cross-check verde) e visive (A/B).

## 9. Testing

- **Host (`cargo test -p ruos-raster`)**: port-unit (rettangolo, testo/uv, alpha,
  damage) + **cross-check bit-identico vs `gui-core`**.
- **Host (`cargo test -p gui-core`)**: i test esistenti restano (gui-core resta per PC).
- **Boot-check (QEMU)**: una finestra `commit_mesh` rasterizzata kernel-side rende
  senza panic; `wm-fps` mostra lo stadio raster.
- **HW reale**: A/B `wm-fps` + visivo su `terminal`.

## 10. Piano a fasi (no big-bang) — vedi `docs/superpowers/plans/2026-06-13-ui-kernel-side-raster.md`

1. Crate `ruos-raster` no_std = port di `raster.rs` (wire structs) + cross-check
   host bit-identico vs `gui-core`. Zero cambi kernel/app.
2. Host fn `wm.tex_update` + `wm.commit_mesh` + stato raster per-finestra (copia in
   kernel). Niente raster ancora (solo store).
3. Kernel: `plan_damage` + raster **seriale** della mesh → surface; `ruos-window`
   converte→wire dietro flag; UNA app (`terminal`) in mesh-mode; A/B vs pixel.
4. `dispatch_raster` SMP a bande; misura HW.
5. Migra le altre app a `commit_mesh`; (opz.) ritira il path pixel.

Più (indipendente, qualunque fase): **Leva #0 repaint scheduling** (egui
`repaint_delay` → `wm.stay_awake`), gira su `wasip1`, taglia l'idle.
