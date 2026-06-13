# UI Parallel Raster — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rendere la rasterizzazione UI egui per-finestra multi-thread (band-parallel, stile llvmpipe), bit-identica al seriale, senza driver GPU.

**Architecture:** Il damage-rect (già calcolato da `Renderer::render`) viene splittato in N bande di righe disgiunte; un kernel per-banda PURO in `gui-core` rasterizza ogni banda; il driver dei thread (split del canvas in slice `&mut` disgiunte + spawn/join) vive in `ruos-window`, usando i thread Fase 2.5. Spike-first: prima si valida che un join cooperativo dentro `frame()` non venga ucciso dal watchdog epoch; altrimenti fallback double-buffer.

**Tech Stack:** Rust, egui/epaint 0.31, tiny-skia 0.11 (`gui-core`), wasm32-wasip1-threads + `std::thread::scope` (`ruos-window`), Wasmtime AOT no_std (kernel), feature `wm-fps` per la misura.

---

## Note di contesto (leggere prima)

- Spec: `docs/superpowers/specs/2026-06-13-ui-parallel-raster-design.md` (AUTORITATIVA).
- Repo doppio: il raster sta nel submodule `ruos-desktop` (`crates/gui-core`, `crates/ruos-window`); il kernel le esegue come `.cwasm`. Build ISO: `make iso` dalla root ruos (via WSL). Test host gui-core: `cargo test -p gui-core` dentro `ruos-desktop` (toolchain stabile Windows, NON serve WSL).
- **Regola d'oro:** `crates/gui-core` importa SOLO egui/epaint/tiny-skia. Il threading NON entra in `gui-core` — solo in `ruos-window`.
- Build threaded window: target `wasm32-wasip1-threads`, flag `--export=__wasi_init_tp` (vedi `Makefile` e spec MT). Gotcha reactor-threads: deadline epoch armata PRIMA dell'instantiate; niente blocking non-cooperativo in `frame()`.
- Comando build singolo tool `.cwasm` threaded (esempio mtwin): vedi target `wt-cwasm`/`build/mtstress.cwasm` nel `Makefile`.

---

## Phase 0 — Baseline su HW reale (misura, da utente)

### Task 0: Catturare i numeri baseline con `wm-fps`

**Files:** nessuno (misura). Risultati → annotare in spec §8.

- [ ] **Step 1: Build ISO con profiling**

Run (WSL, dalla root ruos): `make iso CARGO_FEATURES=wm-fps`
Expected: `build/os.iso` prodotta.

- [ ] **Step 2: Boot su HW reale + workload**

Booti `build/os.iso` sull'i7-11800H. Apri una finestra che ridisegna in continuazione (egui demo dal launcher, o scroll continuo di una lista lunga, o trascina una finestra). Leggi l'overlay in basso a destra: `display: N fps` / `rendering: X ms` / `blit: Y ms`.

- [ ] **Step 3: Registrare i numeri**

Annota in `docs/superpowers/specs/2026-06-13-ui-parallel-raster-design.md` §8 "Baseline (TODO)": `frame_all avg`, `present avg`, `fps`, e il numero di core (`-smp`/online). Questi calibrano i target e quantificano lo speedup atteso.

---

## Phase 1 — SPIKE: join cooperativo dentro `frame()`

Obiettivo: stabilire SE un fan-out + attesa dei worker DENTRO `frame()` (necessario perché il canvas deve essere pronto prima di `wm.commit`) è fattibile senza che il watchdog epoch uccida la finestra. Esito → sceglie il pattern del driver in Phase 4.

### Task 1: Spike thread fan-out/join in `frame()`

**Files:**
- Modify: `tools/mtwin/src/lib.rs` (aggiungere uno stadio spike dietro un contatore di frame; NON rimuovere il gate esistente)

- [ ] **Step 1: Aggiungere uno stadio "parallel-for in frame()" a mtwin**

In `tools/mtwin/src/lib.rs`, dopo lo stage C esistente, aggiungere (usa `std::thread::scope`, che joina a fine scope — è l'attesa-in-frame da validare):

```rust
// Stage D (spike #498): fan-out + join DENTRO frame(). Somma parallela di un
// array, 4 bande, scope join. Misura il tempo via wm::tick non disponibile qui;
// il gate kernel osserva solo che frame() RITORNA (no watchdog kill) e che il
// risultato è corretto (scritto nei byte 8..12 della surface).
static SPIKE_DONE: AtomicBool = AtomicBool::new(false);
if !SPIKE_DONE.swap(true, Ordering::SeqCst) {
    let data: std::vec::Vec<u32> = (0..40_000u32).collect();
    let n = 4usize;
    let chunk = data.len() / n;
    let mut partials = [0u64; 4];
    std::thread::scope(|s| {
        let mut handles = std::vec::Vec::new();
        for k in 0..n {
            let slice = &data[k * chunk..if k == n - 1 { data.len() } else { (k + 1) * chunk }];
            handles.push(s.spawn(move || slice.iter().map(|&x| x as u64).sum::<u64>()));
        }
        for (k, h) in handles.into_iter().enumerate() {
            partials[k] = h.join().unwrap();
        }
    }); // <-- join cooperativo: il fiber chiamante attende i worker
    let total: u64 = partials.iter().sum();
    unsafe { BUF[8..12].copy_from_slice(&(total as u32).to_le_bytes()); }
}
```

- [ ] **Step 2: Build mtwin.cwasm**

Run (WSL, root ruos): `make build/mtwin.cwasm` (o il target che produce `mtwin.cwasm`; vedi `Makefile`).
Expected: `.cwasm` prodotto senza errori.

- [ ] **Step 3: Run headless e osservare**

Run (WSL): build ISO con mtwin lanciato e `wm-fps`, boot headless `-smp 4`, grep dei marker. Comando d'esempio:
`make iso CARGO_FEATURES=wm-fps && timeout 40 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -boot d -cdrom build/os.iso -serial stdio -display none -no-reboot -m 2048 2>&1 | grep -aE "THREADS-WIN-OK|WATCHDOG|panic|wmfps"`

Expected (CRITERIO DI DECISIONE):
- **PASS (cooperativo OK):** `THREADS-WIN-OK` ancora presente, NESSUN `WATCHDOG`/`panic`, e il valore atteso `sum(0..40000)=799_980_000 → u32 wrap` è coerente. → Phase 4 usa il **driver cooperativo** (`std::thread::scope`).
- **FAIL (watchdog kill o hang):** compare `frame() WATCHDOG` o la finestra muore. → Phase 4 usa il **fallback double-buffer** (Task 8b).

- [ ] **Step 4: Documentare l'esito + commit**

Scrivi l'esito (PASS/FAIL + 1 riga di motivo) in cima al Task 8 di questo piano. Poi:
```bash
git add tools/mtwin/src/lib.rs docs/superpowers/plans/2026-06-13-ui-parallel-raster.md
git commit -m "spike(ui): fan-out/join cooperativo in frame() — esito #498"
```

---

## Phase 2 — Leva #0: repaint scheduling (idle quasi-zero)

Indipendente dal raster. Propaga il `repaint_delay` di egui a `wm.stay_awake` così le animazioni restano fluide senza chiamate manuali e l'idle non gira a 100 Hz inutilmente.

### Task 2: Binding `wm.stay_awake` in `ruos-window`

**Files:**
- Modify: `ruos-desktop/crates/ruos-window/src/lib.rs` (blocco `extern "C"` del modulo `wm`)
- Modify: `docs/api/wm.md` (ruos) — aggiungere/confermare l'entry `stay_awake`

- [ ] **Step 1: Verificare se il binding esiste già**

Run (in `ruos-desktop`): `rg "stay_awake" crates/ruos-window/src/lib.rs`
Se presente → salta allo Step 3. Se assente → Step 2.

- [ ] **Step 2: Aggiungere il binding extern**

Nel blocco `#[link(wasm_import_module = "wm")] extern "C" { … }` di `crates/ruos-window/src/lib.rs`, aggiungere:

```rust
    /// Chiede al compositor di tenere sveglia questa finestra il PROSSIMO frame
    /// (modello egui request_repaint). Host fn kernel: `wm.stay_awake`.
    fn stay_awake();
```

- [ ] **Step 3: Chiamare stay_awake quando egui chiede repaint immediato**

In `frame_once` (`crates/ruos-window/src/lib.rs`), dopo `let out = state.ctx.run(...)` e PRIMA del `tessellate`, aggiungere:

```rust
    // Repaint scheduling: se egui vuole ridisegnare subito (animazione/spinner/
    // cursore testo), resta sveglio il prossimo frame. ROOT viewport in egui 0.31.
    let wants_now = out
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .map(|v| v.repaint_delay == core::time::Duration::ZERO)
        .unwrap_or(false);
    if wants_now {
        unsafe { wm::stay_awake(); }
    }
```

- [ ] **Step 4: Idem in `frame_once_bare`**

Applicare la STESSA aggiunta dello Step 3 dentro `frame_once_bare` (stesso punto, dopo il suo `ctx.run`).

- [ ] **Step 5: Aggiornare docs/api/wm.md**

Aprire `docs/api/wm.md` (nel repo ruos). Confermare che esista l'entry `stay_awake()` (signature, semantica: "tieni sveglia la finestra il prossimo frame; equivalente a request_repaint"). Se assente, aggiungerla; aggiornare "Last reviewed" della pagina.

- [ ] **Step 6: Build di verifica + commit**

Run (WSL, root ruos): `make iso` (verifica che gli app finestra ricompilino senza errori).
Expected: ISO prodotta. Poi:
```bash
git add ruos-desktop/crates/ruos-window/src/lib.rs docs/api/wm.md
git commit -m "feat(ui): repaint scheduling — egui repaint_delay -> wm.stay_awake"
```
(NB: `ruos-desktop` è un submodule → committare DENTRO il submodule il file `.rs`, poi nel superprogetto il puntatore submodule + `docs/api/`. Vedi regole submodule.)

---

## Phase 3 — `gui-core`: rasterizzatore band-able (TDD, host, NO thread)

Il cuore. Si fa interamente su host con `cargo test -p gui-core`. Niente kernel, niente thread. Si refactora `raster.rs` perché il passo di raster lavori per-banda; `render()` resta API pubblica invariata (chiama un loop di bande sequenziale). Si aggiunge il test di equivalenza serial↔band bit-identico.

### Task 3: `Band` view + `fill_rect` per-banda

**Files:**
- Modify: `ruos-desktop/crates/gui-core/src/raster.rs`
- Test: `ruos-desktop/crates/gui-core/src/raster.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Introdurre la struct `Band`**

In `raster.rs`, sopra `fn fill_rect`, aggiungere:

```rust
/// Vista mutabile di una banda di righe `[y0, y1)` del canvas (RGBA premoltiplicato),
/// row-major, `width` px per riga. `px.len() == (y1 - y0) * width`. Gli indici sono in
/// coordinate SCHERMO assolute (py in [y0,y1), px in [0,width)); l'offset di riga è
/// rimosso internamente. Due bande con range disgiunti non aliasano → thread-safe.
struct Band<'a> {
    px: &'a mut [PremultipliedColorU8],
    width: i32,
    y0: i32,
    y1: i32,
}

impl Band<'_> {
    #[inline]
    fn get(&self, x: i32, y: i32) -> PremultipliedColorU8 {
        self.px[((y - self.y0) * self.width + x) as usize]
    }
    #[inline]
    fn put(&mut self, x: i32, y: i32, c: PremultipliedColorU8) {
        self.px[((y - self.y0) * self.width + x) as usize] = c;
    }
}
```

- [ ] **Step 2: Riscrivere `fill_rect` per scrivere in una `Band`**

Sostituire l'attuale `fn fill_rect(px: &mut Pixmap, rect, rgba)` con la versione per-banda (clampa la y del rect alla banda):

```rust
/// Riempie il sotto-rettangolo `(x0,y0,x1,y1)` (semiaperto, già clampato allo schermo)
/// con `rgba`, MA solo le righe che ricadono nella banda `[band.y0, band.y1)`.
fn fill_rect(band: &mut Band, rect: (i32, i32, i32, i32), rgba: [u8; 4]) {
    let c = PremultipliedColorU8::from_rgba(rgba[0], rgba[1], rgba[2], rgba[3])
        .unwrap_or_else(|| PremultipliedColorU8::from_rgba(0, 0, 0, 255).unwrap());
    let (x0, y0, x1, y1) = rect;
    let y0 = y0.max(band.y0);
    let y1 = y1.min(band.y1);
    for y in y0..y1 {
        for x in x0..x1 {
            band.put(x, y, c);
        }
    }
}
```

- [ ] **Step 3: Compilare (atteso: errori nei chiamanti)**

Run (in `ruos-desktop`): `cargo build -p gui-core`
Expected: FAIL — `raster_tri` e `render` chiamano ancora le vecchie firme. Si sistemano nei task seguenti.

### Task 4: `raster_tri` scrive in una `Band`

**Files:**
- Modify: `ruos-desktop/crates/gui-core/src/raster.rs` (`fn raster_tri`)

- [ ] **Step 1: Cambiare la firma e il target di `raster_tri`**

In `fn raster_tri`, cambiare il primo parametro da `target: &mut Pixmap` a `band: &mut Band`. Rimuovere le righe:

```rust
    let tw = target.width() as i32;
    let tdata = target.pixels_mut();
```

e sostituire il clamp del bounding box in y perché resti dentro la banda. Dopo le righe che calcolano `miny`/`maxy`:

```rust
    let minx = (x0.min(x1).min(x2).floor() as i32).max(cx0);
    let miny = (y0.min(y1).min(y2).floor() as i32).max(cy0).max(band.y0);
    let maxx = (x0.max(x1).max(x2).ceil() as i32).min(cx1);
    let maxy = (y0.max(y1).max(y2).ceil() as i32).min(cy1).min(band.y1);
    if minx >= maxx || miny >= maxy {
        return;
    }
```

- [ ] **Step 2: Cambiare la scrittura/lettura del pixel nel loop**

Nel doppio loop `for py in miny..maxy { for px in minx..maxx { … } }`, sostituire il blocco finale che indicizza `tdata`:

```rust
            // OVER premoltiplicato sopra il dst.
            let dst = band.get(px, py);
            let inv = 1.0 - fa / 255.0;
            let or = (fr + dst.red() as f32 * inv).round();
            let og = (fg + dst.green() as f32 * inv).round();
            let ob = (fb + dst.blue() as f32 * inv).round();
            let oa = (fa + dst.alpha() as f32 * inv).round();

            let oa_u = oa.clamp(0.0, 255.0) as u8;
            let clampc = |c: f32| c.clamp(0.0, oa).min(255.0) as u8;
            let out_c = PremultipliedColorU8::from_rgba(clampc(or), clampc(og), clampc(ob), oa_u)
                .unwrap_or(dst);
            band.put(px, py, out_c);
```

(Le righe `let idx = (py * tw + px) as usize;` e `let dst = tdata[idx];` e `tdata[idx] = …` spariscono, rimpiazzate da `band.get`/`band.put` sopra. Tutto il resto del corpo — edge function, baricentrico, sampling — INVARIATO.)

- [ ] **Step 2b: Compilare**

Run: `cargo build -p gui-core`
Expected: FAIL ancora — solo `render()` usa le vecchie firme adesso. Prossimo task.

### Task 5: `raster_band` (kernel puro) + `render()` su bande sequenziali

**Files:**
- Modify: `ruos-desktop/crates/gui-core/src/raster.rs` (`Renderer::render` + nuova `raster_band`)

- [ ] **Step 1: Estrarre `raster_band` come fn libera pura**

Aggiungere (vicino a `raster_tri`):

```rust
/// Kernel di rasterizzazione PURO per UNA banda. Disegna `clear` + tutte le `prims`
/// clippate a `(damage ∩ [band.y0, band.y1))` dentro `band`. Niente thread, niente
/// stato globale: due bande con righe disgiunte possono girare in parallelo.
fn raster_band(
    band: &mut Band,
    damage: (i32, i32, i32, i32),
    clear: [u8; 4],
    prims: &[ClippedPrimitive],
    textures: &HashMap<TextureId, Pixmap>,
) {
    fill_rect(band, damage, clear);
    for cp in prims {
        let clip = cp.clip_rect;
        let cx0 = (clip.min.x.floor() as i32).max(damage.0);
        let cy0 = (clip.min.y.floor() as i32).max(damage.1);
        let cx1 = (clip.max.x.ceil() as i32).min(damage.2);
        let cy1 = (clip.max.y.ceil() as i32).min(damage.3);
        if cx0 >= cx1 || cy0 >= cy1 {
            continue;
        }
        match &cp.primitive {
            Primitive::Mesh(mesh) => {
                let tex = textures.get(&mesh.texture_id);
                for tri in mesh.indices.chunks_exact(3) {
                    let v0 = &mesh.vertices[tri[0] as usize];
                    let v1 = &mesh.vertices[tri[1] as usize];
                    let v2 = &mesh.vertices[tri[2] as usize];
                    raster_tri(band, (cx0, cy0, cx1, cy1), tex, v0, v1, v2);
                }
            }
            Primitive::Callback(_) => {}
        }
    }
}
```

- [ ] **Step 2: Riscrivere il blocco di raster dentro `Renderer::render`**

Nel corpo di `render()`, sostituire l'intero blocco `{ let clear = self.clear; let target = self.canvas.as_mut().unwrap(); fill_rect(...); for cp in prims { … } }` con una chiamata band-able a banda singola (l'intero damage):

```rust
        // Raster del damage. Una sola banda = path seriale; il driver parallelo
        // (ruos-window) richiamerà `raster_band` su N bande disgiunte.
        let dmg = (damage.x0, damage.y0, damage.x1, damage.y1);
        {
            let clear = self.clear;
            let Renderer { canvas, textures, .. } = self;
            let cv = canvas.as_mut().unwrap();
            let width = cv.width() as i32;
            let mut band = Band { px: cv.pixels_mut(), width, y0: 0, y1: ih };
            raster_band(&mut band, dmg, clear, prims, textures);
        }
```

(Le `use` esistenti restano; `ih` è già definita all'inizio di `render` come `height as i32`.)

- [ ] **Step 3: Compilare**

Run: `cargo build -p gui-core`
Expected: PASS (warnings ok).

- [ ] **Step 4: Eseguire i test pixel esistenti (non-regressione)**

Run: `cargo test -p gui-core`
Expected: PASS — `renders_solid_red_rect`, `alpha_blends_over_background`, `dirty_rect_move_matches_full_render`, `dirty_rect_recolor_matches_full_render`, `unchanged_scene_yields_empty_dirty` tutti verdi (il refactor è equivalente a banda singola).

- [ ] **Step 5: Commit**

```bash
git add crates/gui-core/src/raster.rs   # dentro ruos-desktop
git commit -m "refactor(gui-core): raster band-able (Band view + raster_band puro)"
```

### Task 6: Helper pubblico per il driver + test equivalenza serial↔band

**Files:**
- Modify: `ruos-desktop/crates/gui-core/src/raster.rs` (API pubblica `render_into_bands` + test)

- [ ] **Step 1: API pubblica che espone il piano + le bande al chiamante**

Il driver in `ruos-window` deve: calcolare damage UNA volta, poi rasterizzare N bande disgiunte. Aggiungere un metodo pubblico che incapsula plan+split ma delega lo SPAWN al chiamante via callback `run` (così `gui-core` resta puro: nessun thread, solo una closure fornita dal chiamante):

```rust
impl Renderer {
    /// Come `render`, ma il raster del damage è suddiviso in `n_bands` bande di
    /// righe disgiunte rasterizzate via `run`, che il CHIAMANTE implementa (seriale
    /// o multi-thread). `run` riceve una closure e DEVE invocarla una volta per ogni
    /// indice di banda `0..n_bands` (in qualunque ordine / in parallelo): le bande
    /// non aliasano. Ritorna `DirtyRect` (w==0 ⇒ niente da presentare).
    ///
    /// Mantiene `gui-core` puro: nessuna dipendenza da thread; il parallelismo è
    /// iniettato dal chiamante (`ruos-window` usa `std::thread::scope`).
    pub fn render_banded<R>(
        &mut self,
        prims: &[ClippedPrimitive],
        deltas: &TexturesDelta,
        width: u32,
        height: u32,
        n_bands: usize,
        run: R,
    ) -> (&Pixmap, DirtyRect)
    where
        R: FnOnce(&mut dyn FnMut(usize, &mut Band)),
    {
        // Riusa la stessa logica di plan/diff di `render` fino al damage; poi
        // splitta. Per evitare duplicazione, `render` stesso è ora il caso n_bands=1
        // (vedi Step 2). Qui sotto la versione generale.
        unimplemented!("vedi Step 2: implementazione")
    }
}
```

- [ ] **Step 2: Implementare `render_banded` (sostituire l'`unimplemented!`)**

Rifattorizzare in modo che `render` deleghi a `render_banded` con `n_bands=1` e un `run` seriale, e `render_banded` contenga la logica di plan/diff + lo split in bande. Implementazione completa di `render_banded` (rimpiazza lo Step 1):

```rust
    pub fn render_banded<R>(
        &mut self,
        prims: &[ClippedPrimitive],
        deltas: &TexturesDelta,
        width: u32,
        height: u32,
        n_bands: usize,
        run: R,
    ) -> (&Pixmap, DirtyRect)
    where
        R: FnOnce(&mut dyn FnMut(usize, &mut Band)),
    {
        let tex_changed = !deltas.set.is_empty() || !deltas.free.is_empty();
        self.apply_textures(deltas);
        let w = width.max(1);
        let h = height.max(1);
        let iw = w as i32;
        let ih = h as i32;
        let realloc = self.canvas.as_ref().map_or(true, |px| px.width() != w || px.height() != h);
        if realloc {
            self.canvas = Some(Pixmap::new(w, h).expect("pixmap alloc"));
        }
        let meta: Vec<PrimMeta> = prims.iter().map(|cp| prim_meta(cp, iw, ih)).collect();
        let full = IRect { x0: 0, y0: 0, x1: iw, y1: ih };
        let mut damage = IRect::empty();
        if realloc || tex_changed || self.prev.len() != meta.len() {
            damage = full;
        } else {
            for (a, b) in self.prev.iter().zip(meta.iter()) {
                if a.hash != b.hash {
                    damage = damage.union(a.bbox).union(b.bbox);
                }
            }
        }
        let is_full = damage.x0 == 0 && damage.y0 == 0 && damage.x1 == iw && damage.y1 == ih;
        if !damage.is_empty() && !is_full {
            damage = IRect { x0: damage.x0 - 1, y0: damage.y0 - 1, x1: damage.x1 + 1, y1: damage.y1 + 1 }.clamp(iw, ih);
        }
        self.prev = meta;
        if damage.is_empty() {
            return (self.canvas.as_ref().unwrap(), DirtyRect { x: 0, y: 0, w: 0, h: 0 });
        }
        let dmg = (damage.x0, damage.y0, damage.x1, damage.y1);

        // Split del damage in n_bands bande di righe (limitato alle righe del damage).
        let n = n_bands.max(1);
        let dy0 = damage.y0;
        let dy1 = damage.y1;
        let rows = (dy1 - dy0) as usize;
        let nb = n.min(rows.max(1)); // mai più bande che righe
        let clear = self.clear;
        {
            let Renderer { canvas, textures, .. } = self;
            let cv = canvas.as_mut().unwrap();
            let width_px = cv.width() as i32;
            // Slice del canvas per le righe del damage: chunk per banda, disgiunti.
            // Ogni banda copre righe [bstart, bend); il resto del canvas non si tocca.
            let band_rows = (rows + nb - 1) / nb; // ceil
            let pixels = cv.pixels_mut();
            // Pre-calcola i range riga di ogni banda.
            let mut ranges: Vec<(i32, i32)> = Vec::with_capacity(nb);
            let mut yy = dy0;
            while yy < dy1 {
                let ye = (yy + band_rows as i32).min(dy1);
                ranges.push((yy, ye));
                yy = ye;
            }
            // Slice disgiunte del buffer pixel per ciascuna banda.
            // pixels copre l'intero canvas: la banda k usa [y0*width .. y1*width).
            // Costruiamo i &mut slice in ordine e li consumiamo nella closure.
            let mut idx = 0usize;
            let mut bands: Vec<Band> = Vec::with_capacity(ranges.len());
            // SAFETY-free: split_at_mut sequenziale per ritagliare ogni banda.
            let mut rest: &mut [PremultipliedColorU8] = pixels;
            let mut consumed = 0i32; // righe già consumate dall'inizio del canvas
            for &(by0, by1) in &ranges {
                // salta le righe prima di by0 (la prima banda parte da dy0, non da 0)
                let skip = (by0 - consumed) as usize * width_px as usize;
                let (_, tail) = rest.split_at_mut(skip);
                let take = (by1 - by0) as usize * width_px as usize;
                let (head, tail2) = tail.split_at_mut(take);
                bands.push(Band { px: head, width: width_px, y0: by0, y1: by1 });
                rest = tail2;
                consumed = by1;
                idx += 1;
            }
            let _ = idx;
            // Il chiamante invoca `each(k, &mut band_k)` per ogni banda.
            // Spostiamo le Band in un Vec<Option<>> per poterle prendere per indice.
            let mut slots: Vec<Option<Band>> = bands.into_iter().map(Some).collect();
            let mut each = |k: usize, _b: &mut Band| {}; // placeholder, vedi nota
            let _ = &mut each;
            // NB: l'API `run` riceve una closure che, dato k, rasterizza la banda k.
            // Per evitare prestiti incrociati, eseguiamo qui col pattern "prendi e
            // restituisci": vedi implementazione driver in ruos-window (Task 7/8).
            run(&mut |k: usize, band: &mut Band| {
                let _ = (k, band);
            });
            // La rasterizzazione vera avviene tramite la closure passata dal chiamante
            // che chiama `raster_band` su ciascuna banda. Vedi Task 7.
            let _ = (&mut slots, dmg, clear, prims, textures, nb);
        }
        (
            self.canvas.as_ref().unwrap(),
            DirtyRect { x: damage.x0 as u32, y: damage.y0 as u32, w: (damage.x1 - damage.x0) as u32, h: (damage.y1 - damage.y0) as u32 },
        )
    }
```

> **Nota di design (da risolvere nel piano in fase di esecuzione):** l'API `render_banded` con callback ha un attrito di borrow (le `Band` mutabili vanno passate al chiamante che le distribuisce ai thread). Due alternative concrete, da scegliere all'esecuzione del Task 6 misurando l'ergonomia:
> - **(6A) callback per-banda `R: Fn(usize) + Sync` con bande pre-splittate restituite** — `gui-core` ritorna `Vec<Band>` (vita legata a `&mut self`) e il chiamante le manda ai thread con `std::thread::scope`. Più diretto.
> - **(6B) `gui-core` espone solo `raster_band` (Task 5) + un helper `band_ranges(damage, n)`** e `ruos-window` fa tutto lo split con `split_at_mut` + scope. `gui-core` NON conosce i thread né la callback. **Preferita** (più pura, meno generics).
>
> **Decisione raccomandata: (6B).** Rifare lo Step 1/2 di questo task come: esporre `pub fn raster_band(...)` (rendere pubblica quella del Task 5) + `pub fn plan_damage(&mut self, prims, deltas, w, h) -> Option<DirtyRect>` che fa plan/diff e ritorna il damage (o None). Lo split + thread vivono in `ruos-window`. Cancellare `render_banded`/`render_into_bands`.

- [ ] **Step 3 (raccomandato 6B): esporre `raster_band` + `plan_damage`, niente callback**

Sostituire l'approccio callback con due API pubbliche pure. Rendere pubblica `raster_band` (già definita nel Task 5) cambiando `fn raster_band` → `pub fn raster_band`, e rendere pubblica `Band` (`pub struct Band` + `pub fn px_slice`/costruttore) così `ruos-window` può costruire le bande. Aggiungere:

```rust
impl Renderer {
    /// Plan+diff: applica le texture, (re)alloca il canvas, calcola il damage rect
    /// del frame. Ritorna `Some(damage)` da rasterizzare, o `None` se niente è
    /// cambiato. Dopo questa chiamata il chiamante rasterizza il damage (seriale o a
    /// bande) via `raster_band`, poi legge `canvas()`.
    pub fn plan_damage(
        &mut self,
        prims: &[ClippedPrimitive],
        deltas: &TexturesDelta,
        width: u32,
        height: u32,
    ) -> Option<(i32, i32, i32, i32)> {
        // (corpo IDENTICO a render fino al calcolo del damage; ritorna None se vuoto)
        // ... vedi Task 5/render per i passi plan/diff ...
        unimplemented!("copiare i passi plan/diff di render(); ritornare Some(dmg) o None")
    }

    /// Canvas corrente (per il commit dopo aver rasterizzato).
    pub fn canvas(&self) -> &Pixmap { self.canvas.as_ref().unwrap() }

    /// Larghezza canvas / colore clear (per costruire le Band lato chiamante).
    pub fn canvas_width(&self) -> i32 { self.canvas.as_ref().map(|p| p.width() as i32).unwrap_or(0) }
    pub fn clear_color(&self) -> [u8; 4] { self.clear }

    /// Accesso mutabile ai pixel del canvas (per lo split in bande lato chiamante).
    pub fn canvas_pixels_mut(&mut self) -> &mut [PremultipliedColorU8] {
        self.canvas.as_mut().unwrap().pixels_mut()
    }
    /// Textures (read-only) per `raster_band`.
    pub fn textures(&self) -> &HashMap<TextureId, Pixmap> { &self.textures }
}
```

> Implementare `plan_damage` copiando i passi plan/diff già presenti in `render` (Task 5), terminando con `Some((damage.x0,damage.y0,damage.x1,damage.y1))` oppure `None`. Poi `render` può essere riscritto come: `match self.plan_damage(...) { None => empty, Some(dmg) => { una Band [0,ih) ; raster_band ; DirtyRect } }`. DRY.

- [ ] **Step 4: Test equivalenza serial↔band (TDD, host)**

Aggiungere al `mod tests` di `raster.rs`:

```rust
    /// Costruisce una scena ricca: wallpaper full-screen + N rettangoli colorati +
    /// un rettangolo "testo-like" con uv variabili (path per-pixel di raster_tri).
    fn rich_scene(w: f32, h: f32) -> Vec<ClippedPrimitive> {
        let clip = Rect::from_min_max(pos2(0.0, 0.0), pos2(w, h));
        let mut v = vec![ClippedPrimitive {
            clip_rect: clip,
            primitive: Primitive::Mesh(rect_mesh(0.0, 0.0, w, h, Color32::from_rgb(10, 20, 30))),
        }];
        for i in 0..6 {
            let x = 4.0 + i as f32 * 9.0;
            v.push(ClippedPrimitive {
                clip_rect: clip,
                primitive: Primitive::Mesh(rect_mesh(x, x, x + 7.0, x + 30.0, Color32::from_rgb(200, 30 + i as u8 * 20, 80))),
            });
        }
        v
    }

    /// Render seriale (1 banda) vs render a 3 bande disgiunte → byte-per-byte uguali.
    /// È la prova che il band-split NON cambia un solo pixel.
    #[test]
    fn banded_matches_serial_bit_identical() {
        let (w, h) = (64u32, 64u32);
        let prims = rich_scene(w as f32, h as f32);
        let deltas = white_texel_delta();

        // Seriale: render normale.
        let mut r_ser = Renderer::new();
        let (px_ser, d_ser) = r_ser.render(&prims, &deltas, w, h);
        let ser = px_ser.data().to_vec();
        assert_eq!((d_ser.w, d_ser.h), (w, h), "primo frame = full");

        // A bande: plan_damage + raster_band su 3 bande disgiunte, sequenziale.
        let mut r_band = Renderer::new();
        let dmg = r_band.plan_damage(&prims, &deltas, w, h).expect("damage full atteso");
        let clear = r_band.clear_color();
        let width = r_band.canvas_width();
        // Split del damage [dmg.1, dmg.3) in 3 bande.
        let (dy0, dy1) = (dmg.1, dmg.3);
        let rows = (dy1 - dy0) as usize;
        let nb = 3usize;
        let band_rows = (rows + nb - 1) / nb;
        // Costruisci ranges.
        let mut ranges = Vec::new();
        let mut yy = dy0;
        while yy < dy1 { let ye = (yy + band_rows as i32).min(dy1); ranges.push((yy, ye)); yy = ye; }
        // Per il test (sequenziale) clona textures fuori dal prestito mutabile.
        let textures = r_band.textures().clone();
        let pixels = r_band.canvas_pixels_mut();
        let mut rest: &mut [PremultipliedColorU8] = pixels;
        let mut consumed = 0i32;
        for &(by0, by1) in &ranges {
            let skip = (by0 - consumed) as usize * width as usize;
            let (_, tail) = rest.split_at_mut(skip);
            let take = (by1 - by0) as usize * width as usize;
            let (head, tail2) = tail.split_at_mut(take);
            let mut band = Band { px: head, width, y0: by0, y1: by1 };
            Renderer::raster_band(&mut band, dmg, clear, &prims, &textures);
            rest = tail2;
            consumed = by1;
        }
        let banded = r_band.canvas().data().to_vec();

        assert_eq!(ser.as_slice(), banded.as_slice(), "band-split ha cambiato dei pixel (atteso bit-identico)");
    }
```

> Nota: il test richiede che `HashMap<TextureId,Pixmap>` sia `Clone` — `Pixmap` è `Clone` in tiny-skia 0.11, `TextureId` pure. Se `raster_band` è un metodo associato (`Renderer::raster_band`) renderlo `pub` come fn associata o libera `pub`. Adeguare il path nel test all'implementazione scelta.

- [ ] **Step 5: Run del test (deve fallire se manca `plan_damage`)**

Run: `cargo test -p gui-core banded_matches_serial_bit_identical -- --nocapture`
Expected: prima dell'implementazione di `plan_damage` → FAIL di compilazione; dopo → PASS.

- [ ] **Step 6: Implementare `plan_damage` (sostituire `unimplemented!`) e far passare**

Copiare i passi plan/diff di `render` dentro `plan_damage` (Step 3), riscrivere `render` per delegare. Poi:
Run: `cargo test -p gui-core`
Expected: PASS (tutti, inclusi i pixel-test esistenti e il nuovo equivalence test).

- [ ] **Step 7: Commit**

```bash
git add crates/gui-core/src/raster.rs   # dentro ruos-desktop
git commit -m "feat(gui-core): API band-raster pura (plan_damage + raster_band pub) + test equivalenza serial-band"
```

---

## Phase 4 — `ruos-window`: driver thread + misura

Dipende dall'esito dello SPIKE (Phase 1). Task 7 = comune (split + driver seriale come fallback/default). Task 8 = variante cooperativa (se spike PASS) oppure double-buffer (se spike FAIL).

### Task 7: Driver di raster a bande in `ruos-window` (seriale prima)

**Files:**
- Modify: `ruos-desktop/crates/ruos-window/src/lib.rs` (`frame_once` + `frame_once_bare`)

- [ ] **Step 1: Aggiungere un helper `raster_windowed` che fa lo split e chiama raster_band**

In `crates/ruos-window/src/lib.rs`, aggiungere (per ora SERIALE — un ciclo, nessun thread; il parallelismo arriva nel Task 8):

```rust
/// Rasterizza il frame nel canvas del renderer, splittando il damage in `n_bands`
/// bande di righe. Versione seriale (ciclo); il Task 8 sostituisce il ciclo con
/// `std::thread::scope` se lo spike è PASS. Ritorna il DirtyRect.
fn raster_windowed(
    renderer: &mut gui_core::raster::Renderer,
    prims: &[egui::ClippedPrimitive],
    deltas: &egui::epaint::textures::TexturesDelta,
    w: u32,
    h: u32,
    n_bands: usize,
) -> gui_core::raster::DirtyRect {
    let dmg = match renderer.plan_damage(prims, deltas, w, h) {
        Some(d) => d,
        None => return gui_core::raster::DirtyRect { x: 0, y: 0, w: 0, h: 0 },
    };
    let clear = renderer.clear_color();
    let width = renderer.canvas_width();
    let (dy0, dy1) = (dmg.1, dmg.3);
    let rows = (dy1 - dy0).max(0) as usize;
    let nb = n_bands.max(1).min(rows.max(1));
    let band_rows = (rows + nb - 1) / nb;
    // ranges
    let mut ranges: Vec<(i32, i32)> = Vec::with_capacity(nb);
    let mut yy = dy0;
    while yy < dy1 { let ye = (yy + band_rows as i32).min(dy1); ranges.push((yy, ye)); yy = ye; }
    // split disgiunto + raster seriale (Task 8 = parallelo)
    let textures = renderer.textures() as *const _; // vedi nota borrow sotto
    let pixels = renderer.canvas_pixels_mut();
    let mut rest: &mut [_] = pixels;
    let mut consumed = 0i32;
    for &(by0, by1) in &ranges {
        let skip = (by0 - consumed) as usize * width as usize;
        let (_, tail) = rest.split_at_mut(skip);
        let take = (by1 - by0) as usize * width as usize;
        let (head, tail2) = tail.split_at_mut(take);
        let mut band = gui_core::raster::Band { px: head, width, y0: by0, y1: by1 };
        // SAFETY: textures non viene mutata durante il raster (read-only).
        let tex = unsafe { &*textures };
        gui_core::raster::Renderer::raster_band(&mut band, dmg, clear, prims, tex);
        rest = tail2;
        consumed = by1;
    }
    gui_core::raster::DirtyRect {
        x: dmg.0 as u32, y: dmg.1 as u32,
        w: (dmg.2 - dmg.0) as u32, h: (dmg.3 - dmg.1) as u32,
    }
}
```

> **Nota borrow:** `canvas_pixels_mut(&mut)` e `textures(&)` confliggono. Pulire in fase di esecuzione esponendo da `gui-core` un unico `canvas_and_textures_mut(&mut self) -> (&mut [PremultipliedColorU8], i32, &HashMap<…>)` che fa il disjoint-borrow via destructuring di `self`, eliminando l'`unsafe`. Aggiungere quel metodo a `gui-core` (Task 6 Step 3) e usarlo qui.

- [ ] **Step 2: Usare `raster_windowed` in `frame_once`**

In `frame_once`, sostituire:
```rust
    let (pixmap, dirty) = state.renderer.render(&prims, &out.textures_delta, w, h);
    if dirty.w == 0 || dirty.h == 0 { return; }
    let data = pixmap.data();
    unsafe { wm::commit(data.as_ptr(), data.len() as u32, w, h); }
```
con:
```rust
    let n_bands = raster_band_count(); // euristica (Step 3)
    let dirty = raster_windowed(&mut state.renderer, &prims, &out.textures_delta, w, h, n_bands);
    if dirty.w == 0 || dirty.h == 0 { return; }
    let data = state.renderer.canvas().data();
    unsafe { wm::commit(data.as_ptr(), data.len() as u32, w, h); }
```
Idem in `frame_once_bare`.

- [ ] **Step 3: Euristica numero bande**

Aggiungere:
```rust
/// Quante bande usare per il raster. Per ora una costante prudente (= core tipici);
/// l'euristica fine (skip-split per damage piccolo, contesa col pool frame()) verrà
/// tarata coi numeri HW reali (spec §5.4). 1 banda = comportamento seriale attuale.
fn raster_band_count() -> usize {
    // Punto di partenza: 4. NON splittare se il damage è piccolo (gestito dentro
    // raster_windowed via nb.min(rows)). Da rendere dinamico in seguito.
    4
}
```

- [ ] **Step 4: Build di verifica (seriale, n_bands>1 ma ancora ciclo)**

Run (WSL, root ruos): `make iso`
Expected: ISO ok. Poi `make run-test` (smoke) o boot headless: la UI deve rendere identica a prima (il ciclo a bande è equivalente al seriale).

- [ ] **Step 5: Commit**

```bash
# dentro ruos-desktop
git add crates/ruos-window/src/lib.rs
git commit -m "feat(ruos-window): driver raster a bande (seriale) + euristica n_bands"
```

### Task 8: Parallelizzare il driver (gated sullo spike)

**ESITO SPIKE (compilare dal Task 1 Step 4):** `____________________`

**Files:**
- Modify: `ruos-desktop/crates/ruos-window/src/lib.rs` (`raster_windowed`)
- Modify: `ruos-desktop/apps/*/Cargo.toml` se serve abilitare i thread (target wasip1-threads già usato dalle finestre MT)

#### Caso 8a — spike PASS: `std::thread::scope` (join cooperativo)

- [ ] **Step 1: Sostituire il ciclo seriale con scope parallelo**

In `raster_windowed`, sostituire il `for &(by0,by1) in &ranges { … raster seriale … }` con:

```rust
    // Raccogli le Band disgiunte in un Vec, poi rasterizzale in parallelo.
    let mut bands: Vec<gui_core::raster::Band> = Vec::with_capacity(ranges.len());
    let mut rest: &mut [_] = pixels;
    let mut consumed = 0i32;
    for &(by0, by1) in &ranges {
        let skip = (by0 - consumed) as usize * width as usize;
        let (_, tail) = rest.split_at_mut(skip);
        let take = (by1 - by0) as usize * width as usize;
        let (head, tail2) = tail.split_at_mut(take);
        bands.push(gui_core::raster::Band { px: head, width, y0: by0, y1: by1 });
        rest = tail2;
        consumed = by1;
    }
    let tex = renderer.textures(); // read-only, Sync
    std::thread::scope(|s| {
        for mut band in bands {
            let prims = &*prims;
            s.spawn(move || {
                gui_core::raster::Renderer::raster_band(&mut band, dmg, clear, prims, tex);
            });
        }
    }); // join cooperativo qui: il fiber chiamante attende le bande
```

(Risolvere il borrow `tex` vs `pixels` con il metodo `canvas_and_textures_mut` della nota del Task 7 Step 1: prendi `(pixels, width, tex)` insieme in un colpo, poi `tex` è un `&` valido per tutta la scope.)

- [ ] **Step 2: Build + boot QEMU (correttezza) e HW reale (perf)**

Run (WSL): `make iso CARGO_FEATURES=wm-fps`. Boot headless `-smp 4`: nessun `WATCHDOG`/`panic`, UI corretta. Poi su HW reale: confronta l'overlay con la baseline (Task 0).
Expected: `frame_all avg` ridotto verso baseline/nb sul caso finestra grande; correttezza invariata.

#### Caso 8b — spike FAIL: double-buffer asincrono (niente blocking in frame())

- [ ] **Step 1: Worker persistenti + due canvas**

Se il join in `frame()` non è praticabile, NON bloccare: `frame()` consegna i `prims` del frame N a worker persistenti (spawnati una volta, come in mtwin) che rasterizzano in un canvas "back", e `frame()` committa il canvas "front" (frame N-1, già pronto), poi swap. Aggiunge 1 frame di latenza ma non blocca mai. Implementazione: struttura `DoubleCanvas { front, back, ready: AtomicBool }`, worker in loop su un channel di job. Dettaglio completo da scrivere all'esecuzione (dipende dalle API thread Fase 2.5 disponibili in `ruos-window`).

- [ ] **Step 2: Build + verifica come 8a Step 2.**

- [ ] **Step 3: Commit (8a o 8b)**

```bash
# dentro ruos-desktop
git add crates/ruos-window/src/lib.rs
git commit -m "feat(ruos-window): raster a bande parallelo (scope cooperativo | double-buffer)"
```

### Task 9: Misura finale + chiusura

- [ ] **Step 1: Numeri prima/dopo su HW reale**

Stesso workload del Task 0. Annota `frame_all avg` baseline vs parallelo + `fps`, e lo speedup, nello spec §8.

- [ ] **Step 2: Aggiornare il puntatore submodule nel superprogetto + changelog**

Nel repo ruos (superprogetto): aggiornare il puntatore `ruos-desktop`, aggiungere una entry CHANGELOG (prossimo NN) che riassume la feature, committare.

- [ ] **Step 3: Verifica completa**

Run (WSL, root ruos): `make run-test` (smoke headless) + `make run-threads-test` (i marker MT, incluso `THREADS-WIN-OK`).
Expected: tutti PASS. La UI su HW reale resta corretta e più veloce.

---

## Self-review note

- Spec §5 (raster band-parallel) → Task 3-8. §5.2 bit-identico → Task 6 Step 4 (test equivalenza). §5.3 purezza gui-core → Task 6 (6B: nessun thread in gui-core). §5.4 euristica N bande → Task 7 Step 3 (parziale; taratura fine rimandata, dichiarata). §5.5 spike → Task 1. §6 (#0 repaint) → Task 2. §7 (#3 SIMD) → fuori scope, nessun task (corretto). §8 misura → Task 0 + Task 9. §9 testing → Task 6 (host), Task 8 (QEMU/HW), Task 1 (spike).
- Placeholder coscienti: Task 6 Step 1/2 contengono un `unimplemented!` SOSTITUITO dallo Step 3 (6B raccomandato) — l'esecutore implementa 6B, non l'API callback. Task 8b è uno sketch perché dipende dall'esito spike + API thread runtime: dichiarato esplicitamente, non un placeholder silenzioso.
- Coerenza tipi: `Band` (pub), `Renderer::raster_band` (pub), `plan_damage`, `canvas()`, `clear_color()`, `canvas_width()`, `DirtyRect` usati coerentemente tra gui-core (def) e ruos-window (uso).
