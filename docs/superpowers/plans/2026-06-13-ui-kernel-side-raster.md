# UI Kernel-Side Raster — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`).

**Goal:** Spostare la rasterizzazione UI egui dal wasm dell'app al kernel (pool SMP), parallela e bit-identica, senza thread per-app e senza regressioni.

**Architecture:** App tessella → `wm.commit_mesh` (wire) → kernel copia → `plan_damage` + `dispatch_raster` (bande SMP) → surface per-finestra → compositor INVARIATO. Spec: `docs/superpowers/specs/2026-06-13-ui-kernel-side-raster-design.md` (AUTORITATIVA).

**Tech Stack:** Rust no_std (`ruos-raster` crate nuova + kernel), egui/epaint 0.31 (solo lato `ruos-window` per tessellare), Wasmtime AOT, pool SMP (`smp::pool`, `dispatch_bands`), feature `wm-fps`.

**Riferimento bit-identico:** `ruos-desktop/crates/gui-core/src/raster.rs` @ commit `184a0d9` (band-able, test equivalenza). Il port deve uguagliarlo byte-per-byte.

---

## Note di lavoro

- Build/test host di `ruos-raster`: dalla root ruos, `cargo test -p ruos-raster` (toolchain WSL: `wsl -d Ubuntu -u root -e bash -lc 'cd /mnt/e/MinimalOS/BasicOperatingSystem && source $HOME/.cargo/env; cargo test -p ruos-raster'`). NB su questa macchina cargo è SOLO in WSL.
- Build ISO/kernel: `make iso` (+ `CARGO_FEATURES=wm-fps` per profilare) via WSL.
- `ruos-raster` deve restare `no_std` + `alloc` (gira nel kernel). Per i test host usa `#![cfg_attr(not(test), no_std)]` o una feature `std`.
- Determinismo: `edge` in **f64**, rounding identico a gui-core. Cross-check obbligatorio.
- Regola d'oro: `ruos-raster` NON dipende da egui/tiny-skia/kernel — solo `core`+`alloc`. Le struct sono il wire format (plain).

---

## Phase 1 — crate `ruos-raster` (port + cross-check host)  [il cuore, TDD host]

### Task 1: scaffold crate `ruos-raster` no_std + wire structs

**Files:**
- Create: `ruos-raster/Cargo.toml`, `ruos-raster/src/lib.rs`
- Modify: workspace `Cargo.toml` di root (membri) se serve (verificare se la root è un workspace; altrimenti crate standalone con path dep).

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "ruos-raster"
version = "0.1.0"
edition = "2021"

[features]
default = []
std = []   # abilita i test host

[dependencies]
```

- [ ] **Step 2: lib.rs scaffold (no_std + alloc)**

```rust
#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;

use alloc::vec::Vec;
use alloc::collections::BTreeMap;

/// Wire vertex (20 B): pos, uv, color (Color32 premoltiplicato, [r,g,b,a]).
#[derive(Clone, Copy)]
pub struct Vertex { pub x: f32, pub y: f32, pub u: f32, pub v: f32, pub color: u32 }

/// Wire primitive (clip rect + texture id + range indici semiaperto).
#[derive(Clone, Copy)]
pub struct Prim { pub clip: [f32; 4], pub tex_id: u64, pub idx0: u32, pub idx1: u32 }

/// Atlante texture: RGBA premoltiplicato, row-major.
pub struct Atlas { pub w: u32, pub h: u32, pub px: Vec<u8> }

/// Stato raster per-finestra (canvas persistente + meta diff + atlanti).
pub struct Raster {
    canvas: Vec<u8>,           // RGBA premoltiplicato, w*h*4
    w: u32, h: u32,
    prev: Vec<PrimMeta>,
    clear: [u8; 4],
    textures: BTreeMap<u64, Atlas>,
}
```

- [ ] **Step 3: build vuoto**

Run: `cargo build -p ruos-raster`  → PASS (scaffold compila).

- [ ] **Step 4: Commit**

`git add ruos-raster Cargo.toml && git commit -m "feat(ruos-raster): scaffold no_std crate + wire structs"`

### Task 2: port di `raster.rs` (fill_rect, raster_tri, raster_band, plan_damage)

**Files:**
- Modify: `ruos-raster/src/lib.rs`
- Riferimento: `ruos-desktop/crates/gui-core/src/raster.rs` @ `184a0d9` (LEGGERLO).

- [ ] **Step 1: Porta i tipi helper**

Sostituisci `tiny_skia::Pixmap` con `canvas: &mut [u8]` (RGBA) e `PremultipliedColorU8` con funzioni inline su `[u8;4]`. Porta `IRect`, `PrimMeta { hash: u64, bbox: IRect }`, `DirtyRect`, `Band { px: &mut [u8], width, y0, y1 }` con `get`/`put` su `[u8;4]` (offset `((y-y0)*width+x)*4`). Porta `prem`, `is_top_left`, `edge` (f64), `sample_bilinear` (su `&[u8]` atlante).

- [ ] **Step 2: Porta `fill_rect`, `raster_tri`, `raster_band` 1:1**

Copia i corpi da gui-core (commit `184a0d9`) cambiando SOLO il plumbing dei tipi
(`Band.put/get` su `[u8;4]`; texture = `&Atlas`/`&[u8]`). La matematica per-pixel
(edge f64, baricentrico, top-left, bilinear, OVER `.round()`, `const_texel`) resta
IDENTICA. `raster_band(band, damage, clear, verts, idx, prims, textures)`.

- [ ] **Step 3: Porta `prim_meta` + `plan_damage`**

`prim_meta(prim, verts, idx, iw, ih) -> PrimMeta` (hash FNV su clip+vertici+indici+
tex_id, bbox = estensione vertici ∩ clip clampata). `Raster::plan_damage(verts, idx,
prims, w, h) -> Option<(i32,i32,i32,i32)>` come gui-core (realloc canvas, diff vs
`prev`, 1px pad, salva `prev`, None se vuoto).

- [ ] **Step 4: `Raster::render` (1 banda, seriale) + `raster_band` pub + apply_tex**

`Raster::set_texture(id, x,y,w,h, px)` (= `set_texture`/patch di gui-core).
`Raster::render(verts, idx, prims, w, h)` = `plan_damage` + una `Band` full-height +
`raster_band` (per i test + il path seriale kernel). `pub fn raster_band(...)` libera.
`canvas(&self) -> &[u8]`.

- [ ] **Step 5: Unit test porta (host, feature std)**

Porta i test pixel di gui-core (rosso pieno, alpha-blend, dirty-rect move/recolor,
unchanged) in `#[cfg(test)]`, adattati al wire format. Run:
`cargo test -p ruos-raster --features std` → PASS.

- [ ] **Step 6: Commit**

`git commit -am "feat(ruos-raster): port raster (fill_rect/raster_tri/raster_band/plan_damage) + unit test"`

### Task 3: cross-check bit-identico vs `gui-core` (la guardia anti-regressione)

**Files:**
- Create: `ruos-raster/tests/crosscheck.rs` (test host che dipende ANCHE da `gui-core`)
- Modify: `ruos-raster/Cargo.toml` (`[dev-dependencies] gui-core = { path = "../ruos-desktop/crates/gui-core" }`, egui, tiny-skia, sotto feature std)

- [ ] **Step 1: Test cross-check**

Costruisci una scena egui (wallpaper + rect colorati + un rect testo-like con uv
variabili + un rect semi-trasparente che attraversa i bordi). Render con
`gui_core::raster::Renderer` (tiny-skia) → `bytes_a`. Converti la STESSA scena nel
wire format (`Vertex`/`Prim`) → render con `ruos_raster::Raster::render` → `bytes_b`.
`assert_eq!(bytes_a, bytes_b)` byte-per-byte. È il golden guard del port.

```rust
// pseudo: usa egui::epaint per costruire ClippedPrimitive, tessellate non serve
// (puoi costruire mesh a mano come fa il mod tests di gui-core: rect_mesh).
// Converti egui Vertex/ClippedPrimitive → ruos_raster Vertex/Prim 1:1 (stessi f32).
```

- [ ] **Step 2: Run**

`cargo test -p ruos-raster --features std` → cross-check PASS (byte-identico).
Se FALLISCE: il port diverge → confronta pixel, NON rilassare l'assert. È un bug del port.

- [ ] **Step 3: Commit**

`git commit -am "test(ruos-raster): cross-check bit-identico vs gui-core (golden)"`

---

## Phase 2 — ABI mesh + stato raster per-finestra (kernel, niente raster ancora)

### Task 4: host fn `wm.tex_update` + `wm.commit_mesh` + buffer kernel

**Files:**
- Modify: `kernel/src/wasm/wt/wm.rs` (linker `wm`: nuove `func_wrap`; struct `Window` + `WinState`)
- Modify: `kernel/Cargo.toml` (`ruos-raster = { path = "../ruos-raster" }`)
- Modify: `docs/api/wm.md`

- [ ] **Step 1: dipendenza + stato per-finestra**

`kernel/Cargo.toml`: aggiungi `ruos-raster`. In `Window` (o stato collegato) aggiungi:
`raster: ruos_raster::Raster`, `mesh_verts: Vec<u8>`, `mesh_idx: Vec<u8>`,
`mesh_prims: Vec<u8>`, `mesh_dirty: bool`, `mesh_mode: bool`.

- [ ] **Step 2: `wm.tex_update`**

`func_wrap("wm","tex_update", |caller, id:u64,x,y,w,h:u32, ptr,len:u32| -> i32)`:
`mem::read` i `len` byte, chiama `win.raster.set_texture(id,x,y,w,h, &px)`. Ritorna 0/28.

- [ ] **Step 3: `wm.commit_mesh`**

`func_wrap("wm","commit_mesh", |caller, vp,vl, ip,il, pp,pl, w,h| -> i32)`: `mem::read`
i tre buffer, COPIA in `win.mesh_verts/idx/prims`, set `win.mesh_dirty=true`,
`win.mesh_mode=true`, salva `w,h`. Ritorna 0. (NESSUN raster qui — solo store.)

- [ ] **Step 4: docs/api/wm.md** — aggiungi `tex_update` e `commit_mesh` (signature,
  wire format, semantica). Aggiorna "Last reviewed".

- [ ] **Step 5: build** `make iso` → compila (le host fn esistono, ancora nessuna app le usa). Commit.

---

## Phase 3 — raster kernel SERIALE + una app in mesh-mode (A/B)

### Task 5: `ruos-window` converte egui→wire dietro flag + path mesh

**Files:**
- Modify: `ruos-desktop/crates/ruos-window/src/lib.rs` (binding extern `tex_update`/`commit_mesh`; conversione tessellato→wire; flag mesh-mode)

- [ ] **Step 1: binding extern + conversione**

Aggiungi `fn tex_update(...)`, `fn commit_mesh(...)` al blocco `extern "wm"`. In
`frame_once`: dopo `ctx.tessellate`, se mesh-mode: per ogni `TexturesDelta` →
`tex_update`; appiattisci `ClippedPrimitive`→ wire (vertici/indici/prims) →
`commit_mesh`. Altrimenti path pixel attuale (`Renderer::render`+`commit`). Flag:
costante/env `RUOS_MESH_MODE` o per-app.

- [ ] **Step 2: build app** terminal → `.cwasm`. Verifica che compili.

### Task 6: kernel raster seriale della mesh → surface

**Files:**
- Modify: `kernel/src/wasm/wt/wm.rs` (dopo `frame_all`, per le finestre mesh-mode con `mesh_dirty`: `plan_damage` + raster seriale → `surface`; `compose_window` usa `surface`)

- [ ] **Step 1: stadio raster (seriale)**

Dopo `dispatch_frames`/Fase C del frame: per ogni finestra `mesh_mode && mesh_dirty`:
parse buffer → `verts/idx/prims` slices → `win.raster.render(...)` (seriale, 1 banda) →
copia/usa `win.raster.canvas()` come `surface`. Reset `mesh_dirty`.

- [ ] **Step 2: `compose_window` per mesh-mode** usa `win.surface` (kernel raster)
  invece della surface committata-pixel.

- [ ] **Step 3: A/B** Build `make iso CARGO_FEATURES=wm-fps`. Boot QEMU: terminal in
  mesh-mode rende identico al pixel-mode (visivo) + nessun panic. Poi HW reale: A/B
  `wm-fps`. **Avanti solo se identico.** Commit.

---

## Phase 4 — parallelizza il raster (SMP) + misura

### Task 7: `dispatch_raster` a bande sul pool SMP

**Files:**
- Modify: `kernel/src/wasm/wt/wm.rs` (clone di `dispatch_bands` per il raster)

- [ ] **Step 1:** `dispatch_raster(win)`: `plan_damage` (core GUI) → split damage in N
  bande → `smp::pool::submit` un job per banda che chiama `ruos_raster::raster_band`
  sui buffer mesh kernel + slice surface disgiunta → join work-steal. 1-core inline.
- [ ] **Step 2:** sostituisci il raster seriale del Task 6 con `dispatch_raster`.
- [ ] **Step 3:** Build wm-fps, boot `-smp 4` (correttezza, no watchdog/panic). Misura HW reale: `frame_all` (ora tessellation) ↓, nuovo stadio raster parallelo. Commit.

### Task 8: strumenta `wm-fps` con lo stadio raster

- [ ] **Step 1:** aggiungi al report `wm-fps` un contatore `raster avg=..us` attorno a
  `dispatch_raster` (come `pr_sum`/`fa_sum`). Commit.

---

## Phase 5 — migrazione + chiusura

### Task 9: migra le altre app + (opz.) ritira il path pixel

- [ ] **Step 1:** abilita mesh-mode per files/system/notepad/about/notify/shell;
  verifica ognuna (visivo + boot). Una alla volta.
- [ ] **Step 2:** quando tutte ok, valuta se ritirare `wm.commit` (pixel) o tenerlo per
  compat. Aggiorna `docs/api/wm.md`.
- [ ] **Step 3:** aggiorna puntatore submodule + CHANGELOG; `make run-test` +
  `make run-threads-test` PASS. Misura finale HW + annota nello spec §8.

---

## Phase 0 / indipendente — Leva #0 repaint scheduling

### Task 10: `repaint_delay` egui → `wm.stay_awake`

**Files:** `ruos-desktop/crates/ruos-window/src/lib.rs`, `docs/api/wm.md`

- [ ] In `frame_once`/`frame_once_bare`, dopo `ctx.run`: se
  `out.viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.repaint_delay == Duration::ZERO)`
  → `unsafe { wm::stay_awake() }`. Binding extern se assente. docs/api. Gira su
  `wasip1`, indipendente da C. Build + commit. (Bonus baseline su HW: Task 0 = boot
  `wm-fps`, annota i numeri.)

---

## Self-review

- Spec §3 flusso → Task 4-7. §4.3 ruos-raster → Task 1-3. §4.2 ABI → Task 4. §5 wire
  → Task 1/5. §6 regression-safety: path doppio (Task 4-6 flag), cross-check (Task 3),
  A/B (Task 6 Step 3), perf (Task 7). §7 rischi: copia-in-kernel (Task 4 Step 3),
  determinismo (Task 2-3). §8 misura → Task 7-8 + Task 9. §9 testing → Task 3/5/6.
- Placeholder coscienti: il port (Task 2) referenzia i corpi di gui-core @184a0d9
  invece di ri-incollarli (700 righe) — la sorgente è committata e il cross-check
  (Task 3) garantisce l'uguaglianza; NON è un "TODO", è un port 1:1 verificato.
- Coerenza tipi: `Vertex`/`Prim`/`Atlas`/`Raster`/`Band`/`raster_band`/`plan_damage`
  usati coerentemente tra ruos-raster (def), kernel (uso), wire format (§5).
