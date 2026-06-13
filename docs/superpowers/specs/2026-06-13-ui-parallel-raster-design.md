# Design — Rasterizzatore UI software parallelo (stile llvmpipe), GPU-less

> **⚠️ SUPERSEDED (2026-06-13)** dall'architettura **kernel-side display-server**:
> `2026-06-13-ui-kernel-side-raster-design.md` (Opzione C). Motivo: le app finestra
> sono `wasm32-wasip1` (single-thread); parallelizzare il raster DENTRO l'app
> richiedeva convertirle tutte a `wasip1-threads` (overhead + rischio). C sposta il
> raster nel kernel (pool SMP esistente), niente thread per-app. **Resta valido di
> questa spec:** il profiling, il prior-art, e il refactor `gui-core` band-able
> (commit `ee39f15`/`184a0d9`) — ora **riferimento bit-identico** per il port kernel
> e raster dell'anteprima PC. Lo spike "join in frame()" (PASS) non serve più.

**Data:** 2026-06-13
**Stato:** SUPERSEDED → vedi `2026-06-13-ui-kernel-side-raster-design.md`
**Topic:** sotto-progetto #1 della direzione "fluidità UI come Ubuntu, in Rust"

## 0. Da dove nasce (contesto del pivot)

La richiesta iniziale era un **driver GPU Intel** per accelerare i calcoli della UI.
Analisi (questa chat):

- La iGPU del target reale (i7-11800H, MSI) è `8086:9A60` = **Tiger Lake, Gen12
  Xe-LP**: execlist-capable ma ISA EU Xe-LP (la peggiore da assemblare), testabile
  **solo su HW reale** (QEMU non emula iGPU Intel). Un driver 3D completo lì è una
  montagna pluriennale per un dev solo.
- Il costo dominante della UI **non** è il compositing — è la **rasterizzazione
  vettoriale** (egui → triangoli → pixel). Un blitter 2D GPU non la tocca; solo un
  backend 3D la accelererebbe (→ il driver mostruoso).
- **Ubuntu "fluido senza GPU" = architettura, non potenza GPU**: quando gira su
  `llvmpipe` (rasterizzatore software **multi-thread + SIMD**), resta usabile grazie
  a retained-mode + damage tracking + raster parallelo su tutti i core.

**Decisione:** replicare lo stack-fluidità software di Ubuntu, in Rust, dentro ruos.
Niente driver GPU per ora.

## 1. Cosa ruos HA GIÀ (prior art — NON rifare)

Profiling con la feature `wm-fps` (TCG, quindi solo ordini di grandezza/rapporti) +
lettura del codice hanno mostrato che il **compositor kernel-side è già "alla
Ubuntu"**:

| Pezzo | Dove | Stato |
|---|---|---|
| Retained per-window surfaces | `wm.rs` (`compose_window`) | ✅ |
| Compositing SMP a bande | `wt/compose.rs` (`composite_band`) | ✅ `composite cores=4` |
| Double buffer | `wm.rs` (`self.backbuf`) | ✅ |
| Present diff-per-riga (damage al present) | `gfx::blit` (RAM shadow + memcmp) | ✅ |
| Idle-skip del present | `wm.rs` (gate `dirty || any_committed`) | ✅ `present=0/s` da idle |
| **Damage-driven RASTER** (canvas persistente + diff hash/bbox per-primitiva) | `ruos-desktop/crates/gui-core/src/raster.rs` (`Renderer::render`) | ✅ con test pixel-equivalenza |
| Frame() per-finestra in parallelo | `wm.rs` (`dispatch_frames` sul pool SMP) | ✅ una finestra ↔ un core |
| Dormienza finestre idle (skip `frame()`) | `wm.rs` (`compute_awake`/`should_wake` + `wm.stay_awake`) | ✅ |

**Conseguenza:** il sotto-progetto #1 NON tocca il compositor. Lavora **dentro la
rasterizzazione della singola finestra**, dove sta l'unico gap rispetto a llvmpipe.

## 2. Profiling (baseline, TCG — solo rapporti)

QEMU qui è TCG (no KVM) → ms assoluti gonfiati ~10-50× e irregolari. Validi solo
come rapporti/struttura. Numeri assoluti veri = overlay `wm-fps` su HW reale (TODO).

- Idle (1 finestra, 4 core): `loop 100/s`, `present 0/s` (idle-skip ok),
  `frame_all ≈ 800-1000 µs/iter`, `present ≈ 0`.
- Blip attivo (primo paint): `frame_all ≈ 20000 µs` vs `present ≈ 4600-6600 µs`
  → **raster ≈ 3-4.5× il present**.

Letture chiave:

1. `present`/composite è **economico, SMP, idle-skipped** → non è il collo.
2. `frame_all` (= `ctx.run` egui + tessellate + raster) **domina**, ed è
   **single-thread per finestra** (`jobs=1` → un solo core).
3. `frame_all` costa ~900 µs **anche da idle**: con damage vuoto il raster esce
   subito, ma `ctx.run` + tessellate girano comunque a 100 Hz.

## 3. Obiettivo e non-obiettivi

**Obiettivo:** portare la rasterizzazione per-finestra da single-thread a
**multi-thread (band-parallel, stile llvmpipe)**, mantenendo il risultato
**bit-identico** al path seriale e `gui-core` **puro** (Regola d'oro). Più, come
polish, tagliare il costo idle propagando il repaint scheduling di egui.

**Non-obiettivi (v1):**

- Niente driver GPU (Intel o altro). Niente `virtio-gpu`.
- Niente SIMD manuale in `raster_tri` (leva #3) — rimandata (vedi §7).
- Niente cambiamenti al compositor kernel-side, al present, al damage-present.
- Niente nuovo damage-tracking (già esiste in `raster.rs`).

## 4. Le leve (ranking corretto dopo lettura codice)

| Leva | Cosa | Stato | In v1? |
|---|---|---|---|
| **#1 raster band-parallel** | split del damage in N bande, una per core | da fare | **SÌ — cuore** |
| **#0 repaint scheduling** | `egui repaint_after` → `wm.stay_awake`/wake-at-deadline | infra parziale (`stay_awake` esiste) | **SÌ — polish** |
| #2 damage-driven raster | re-raster solo del dirty rect | ✅ già fatto | no (riuso) |
| #3 SIMD in `raster_tri` | vettorializzare il loop per-pixel | da fare, grosso | **NO — futuro** |

## 5. Design — Leva #1: rasterizzatore a bande parallelo

### 5.1 Flusso attuale di `Renderer::render` (sequenziale)

1. `apply_textures(deltas)` — cheap.
2. meta per-primitiva (hash + bbox) — cheap, sequenziale.
3. `damage` IRect = unione bbox (vecchio ∪ nuovo) delle primitive con hash cambiato.
4. damage vuoto → return (nessun raster).
5. **[IL COSTO]** `fill_rect(canvas, damage, clear)` + per ogni prim, per ogni
   triangolo: `raster_tri(canvas, clip∩damage, …)`.
6. return `(&canvas, DirtyRect)`.

### 5.2 Flusso parallelo

Passi 1-4 restano **sequenziali** (sono economici). Solo il passo 5 si parallelizza:

- Splitta `damage` in **N bande orizzontali** disgiunte `[by0, by1)`.
- Per ogni banda, un **kernel puro** `raster_band(band_slice, band_clip, prims,
  textures)` esegue `fill_rect` della banda + `raster_tri` di ogni triangolo
  clippato a `banda ∩ damage`.
- Le bande scrivono **range di righe disgiunti** dello stesso canvas → nessun
  aliasing → eseguibili su N thread senza sincronizzazione (identico al pattern di
  `composite_band` nel kernel).

**Perché è bit-identico al seriale:** `raster_tri` legge il `dst` (per l'OVER-blend)
solo del pixel che scrive; un pixel appartiene a una sola banda; un triangolo che
attraversa più bande viene rasterizzato in ciascuna banda clippato alle sue righe →
ogni pixel scritto esattamente una volta dalla banda proprietaria. Nessuna dipendenza
cross-banda. La copertura top-left e l'edge-function sono deterministiche per pixel.

### 5.3 Purezza di `gui-core` (Regola d'oro)

`gui-core` può importare **solo** egui/epaint/tiny-skia — **non** rayon/std-thread.
Quindi il threading vive nel chiamante (`ruos-window`, on-device; `pc-backend`,
anteprima). Split proposto:

- `gui-core::raster` espone:
  - `Renderer::plan(prims, deltas, w, h) -> Option<RenderPlan>` — passi 1-4; `None`
    se damage vuoto. `RenderPlan` tiene damage + i riferimenti a prims/textures.
  - `Renderer::raster_band(plan, canvas_band: &mut [u8], band_y0, band_y1)` — kernel
    puro per una banda (passo 5 ristretto alle righe della banda). Niente thread.
  - `Renderer::finish(plan) -> DirtyRect`.
- Il **driver dei thread** (split del buffer canvas in N `&mut [u8]` disgiunti per
  righe, spawn/join) sta in `ruos-window` (usa i thread Fase 2.5) e in `pc-backend`
  (usa rayon/std). `gui-core` resta puro.

Alternativa (se lo split del `&mut Pixmap` in slice disgiunte risulta scomodo con
l'API tiny-skia): API tipo `composite_band` con raw pointer + `unsafe` documentato,
incapsulata dentro `raster_band` (il chiamante passa solo `(ptr, stride, y0, y1)`).
Da decidere in fase di piano sulla base dell'API `Pixmap::pixels_mut`/`data_mut`.

### 5.4 Numero di bande e interazione col pool SMP

- N bande ≈ core disponibili (cap, es. 8). Per finestre piccole/damage piccolo,
  N=1 (lo split non ripaga sotto una soglia di righe/pixel — euristica da tarare).
- **Contesa col pool frame()**: oggi `dispatch_frames` dà ~1 core a finestra. Se UNA
  finestra parallelizza la sua raster, usa i core liberi (caso `jobs=1` = ideale).
  Con molte finestre sveglie, ognuna ha già ~1 core → il band-parallel rende poco e
  potrebbe contendere. Euristica: band-split aggressivo quando `jobs` basso, degradare
  a seriale quando il pool è saturo. Dettaglio in fase di piano.

### 5.5 Premessa CRITICA da validare per prima (spike)

I thread Fase 2.5 girano nelle finestre (provato: `THREADS-WIN-OK`, `tools/mtwin`),
ma la memoria nota **"niente blocking in `frame()`"** (un job non-fiber fa hlt-wait
fuori dal watchdog). Un `rayon::join`/parallel-for DENTRO `frame()` blocca il
chiamante finché le bande finiscono. **Serve uno spike** che provi un parallel-for
cooperativo (work-steal) dentro `frame()` su `tools/mtwin`, misurando lo speedup e
verificando che il watchdog epoch non uccida la finestra. Se non fattibile
cooperativamente, fallback: la finestra calcola le bande su worker persistenti
sincronizzati per frame (latch), evitando il join bloccante in `frame()`.
**Questo spike è il Task 1 del piano; il resto dipende dal suo esito.**

Vedi spec MT: `docs/superpowers/specs/2026-06-12-wasm-mt-fase2-threads-design.md`
e `2026-06-13-wm-threaded-windows-design.md`.

## 6. Design — Leva #0: repaint scheduling (polish idle)

Oggi `compute_awake`/`should_wake` tengono sveglia una finestra se: ha eventi
input, ha chiamato `wm.stay_awake()`, o è entro il grace dall'ultima attività.
egui, da `ctx.run`, ritorna un **`repaint_after: Duration`** (0 = subito, grande =
idle). Gap: `ruos-window::frame_once` NON traduce `repaint_after` in
`wm.stay_awake()`.

Proposta:

- In `frame_once`/`frame_once_bare`: se `out.repaint_after == 0` (animazione in
  corso), chiama `wm.stay_awake()`. Così le animazioni egui (spinner, fade hover,
  cursore testo) restano fluide senza che l'app lo faccia a mano.
- (Opzionale, fase 2) wake-at-deadline preciso: passare `repaint_after` al kernel
  (`wm.request_frame_at(ms)`) così il compositor riarma un timer invece di girare a
  100 Hz fisso. Riduce il costo idle quando egui chiede "ridisegna tra 500 ms".
  Modesto su HW reale (la dormienza già copre il caso comune) → valutare coi numeri.

## 7. Leva #3 (SIMD) — esplicitamente fuori dalla v1

L'hot path è `raster_tri` scritto a mano (edge-function f64, blend f32 per-pixel),
NON tiny-skia → `-C target-feature=+simd128` da solo non aiuta. SIMD vero =
riscrivere `raster_tri` a processare 4-8 pixel insieme (maschera di copertura
vettoriale + blend packed). Vincoli: deve restare portabile (no intrinsics x86 in
`gui-core`; semmai `core::arch::wasm32` v128 dietro feature, o crate `wide`) e
**bit-identico** (rischio: l'ordine delle operazioni f32 cambia i risultati →
romperebbe i test pixel e l'equivalenza seriale/parallela). Rimandata a un
sotto-progetto successivo, dopo che #1 ha dato lo speedup multi-core.

## 8. Misura e criteri di successo

- Strumento: feature `wm-fps` (già esistente) → overlay a schermo + riga `wmfps`
  (`frame_all avg/max µs`, `present avg µs`, `fps`, `iters/s`, `jobs`).
- **La misura che conta è su HW reale** (i7-11800H). Workload: una finestra che
  ridisegna in continuazione (egui demo / scroll di una lista lunga / drag).
- **Baseline (TODO — riempire con numeri HW reali):**
  - `frame_all avg` single-thread, finestra sotto carico: `___ µs`
  - `present avg`: `___ µs`
  - `fps` percepiti: `___`
- **Target v1 (band-parallel):**
  - speedup `frame_all` ≈ **lineare nei core fino a ~N/2** sul caso `jobs=1` finestra
    grande (es. 4 core → ≥ 2.5× sul raster; numero esatto dopo baseline).
  - nessuna regressione di correttezza: test pixel `gui-core` e equivalenza
    seriale↔banda **bit-identici**.
  - idle (#0): `frame_all` idle → ~0 quando egui non chiede repaint.

## 9. Testing

- **`gui-core` (host, `cargo test -p gui-core`)**: estendere `raster.rs` con un test
  **equivalenza seriale↔banda**: stesso `prims`/`deltas`, render seriale vs render a
  N bande → `assert_eq!` byte-per-byte sul canvas (come `dirty_rect_*` esistenti).
  I test pixel attuali (`renders_solid_red_rect`, `dirty_rect_*`) restano verdi.
- **On-device (QEMU, sanity)**: build con `wm-fps`, boot headless, confermare che la
  finestra threaded raster non panica e che il watchdog non uccide (TCG: solo
  funzionale, NON perf).
- **On-device (HW reale, perf)**: overlay `wm-fps` prima/dopo, stesso workload.
- **Spike (Task 1)**: `tools/mtwin` con parallel-for in `frame()` → speedup misurato
  + nessun kill watchdog.

## 10. Rischi

1. **rayon/parallel-for bloccante in `frame()`** (§5.5) — rischio #1, mitigato dallo
   spike-first e dal fallback a worker latch-sincronizzati.
2. **Purezza `gui-core`** — mitigato tenendo il threading in `ruos-window`/`pc-backend`
   e il kernel per-banda puro in `gui-core`.
3. **Determinismo bit-identico** — mitigato dal test di equivalenza; il band-split
   non cambia l'ordine delle op per-pixel (ogni pixel scritto una volta).
4. **Contesa col pool frame()** multi-finestra (§5.4) — mitigato dall'euristica
   band-split-quando-`jobs`-basso.
5. **TCG non valida la perf** — mitigato delegando i numeri all'HW reale; QEMU resta
   per la correttezza.
6. **Doppio repo** (kernel ruos + submodule `ruos-desktop`) — le modifiche raster
   stanno nel submodule (`gui-core`/`ruos-window`); il Makefile padre referenzia i
   path → tenere in lockstep. `docs/api/` da aggiornare se cambiano host fn `wm`.

## 11. Sequenza (per il piano)

1. **Spike** rayon parallel-for in `frame()` su `tools/mtwin` (valida §5.5).
2. **Leva #0** repaint scheduling (`frame_once` → `stay_awake` su `repaint_after==0`)
   — piccolo, indipendente, riduce subito l'idle.
3. **Leva #1**: refactor `raster.rs` in `plan`/`raster_band`/`finish` + test
   equivalenza seriale↔banda (host) → driver thread in `ruos-window` → euristica
   N bande → misura HW reale.
4. (Futuro) **Leva #3** SIMD `raster_tri`.

## 12. Repo / file toccati (previsti)

- `ruos-desktop/crates/gui-core/src/raster.rs` — split `plan`/`raster_band`/`finish`
  + test equivalenza.
- `ruos-desktop/crates/ruos-window/src/lib.rs` — driver thread del raster a bande +
  `repaint_after` → `stay_awake`.
- (eventuale) `kernel/src/wasm/wt/wm.rs` — solo se si fa il wake-at-deadline (#0 fase 2).
- `tools/mtwin/` — spike.
- `docs/api/` — se cambiano host fn `wm`.
