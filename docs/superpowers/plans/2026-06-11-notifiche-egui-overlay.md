# Notifiche egui overlay + panic screen — Piano di implementazione

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** toast + modale power renderizzati in egui vero da una app overlay
full-screen trasparente (`notify`), decor kernel come fallback, panic screen
kernel-side direct-framebuffer per i FATAL.

**Architettura:** flag `overlay` speculare a `bg` (z-top, alpha-blend nel band
kernel, hit-test per-pixel sull'alpha); eventi alla app via `sys.events_poll`
(record 64 B, cursore per-finestra sul kevent ring già esistente); countdown da
`wm.power_pending` + `wm.power_cancel`; panic handler disegna direttamente sul
framebuffer (atomics gfx, no lock).

**Tech stack:** kernel Rust no_std (wm.rs/compose.rs/gfx), Wasmtime AOT,
egui 0.31 + tiny-skia (ruos-desktop), SDK ruos-window.

**Spec (AUTORITATIVA):** `docs/superpowers/specs/2026-06-11-notifiche-egui-overlay-design.md`
**Prerequisito:** kernel event bus v1 implementato (CHANGELOG 471).

**Regole vincolanti (CLAUDE.md):** NIENTE commit/push non richiesti (checkpoint
= build/test verdi). Build via WSL:
`wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && <cmd>'`
(abbreviato `WSL: <cmd>`). Host fn nuove → docs/api nello STESSO task.
Changelog a fine lavoro. Submodule `ruos-desktop`: modifiche locali, niente commit.

**Nota deviazione spec (§4):** non serve `frame_once_overlay` — basta
`WindowState::new_overlay()` (clear trasparente) + `frame_once_bare` esistente.
Il Task 9 aggiorna la spec di conseguenza.

---

### Task 1: alpha-blend nel band kernel (`compose.rs` + arena)

**Files:**
- Modify: `kernel/src/wasm/wt/compose.rs:24-32` (WinDesc) e `:90-150` (composite_band)
- Modify: `kernel/src/wasm/wt/wm.rs:435-437` (WIN_ARENA init), `:1633-1646` (descs), `:1719-1721` (WinDesc literal)

- [ ] **Step 1: campo `blend` in `WinDesc`**

In `compose.rs` aggiungi il campo alla struct:

```rust
#[derive(Copy, Clone)]
pub struct WinDesc {
    pub px: *const u8, // footprint base (RGBA8888, src_stride = w*4)
    pub px_len: usize, // footprint length in bytes (bounds guard)
    pub x: u32,        // on-screen top-left x
    pub y: u32,        // on-screen top-left y
    pub w: u32,        // footprint width  (px)
    pub h: u32,        // footprint height (px)
    pub shadow: bool,  // cast a drop shadow under this window (false for the bg)
    /// Alpha-blend (src-over, sorgente PREMOLTIPLICATA — tiny-skia) invece di
    /// copy opaco. Solo la finestra overlay (notifiche) lo usa; tutto intero,
    /// così il composite a bande resta bit-identico al riferimento seriale.
    pub blend: bool,
}
```

- [ ] **Step 2: ramo blend in `composite_band`**

In `composite_band`, sostituisci il blocco della copia riga (righe ~136-148,
il `while sy < y_end { ... copy_nonoverlapping ... }`) con:

```rust
        let mut sy = y_start;
        while sy < y_end {
            let src_row_off = (sy - wy) * src_stride;
            let src_end = src_row_off + vis_w * 4;
            if src_end > win.px_len { break; }
            let src = win.px.add(src_row_off);
            let dst = back.add(sy * stride + wx * 4);
            if win.blend {
                // Overlay: src-over con sorgente premoltiplicata (tiny-skia).
                // out = src + dst*(255-a)/255 per canale; X del back-buffer
                // ignorato. Tutto intero (equivalenza seriale/parallela).
                let mut i = 0usize;
                while i < vis_w {
                    let o = i * 4;
                    let a = *src.add(o + 3) as u32;
                    if a == 255 {
                        core::ptr::copy_nonoverlapping(src.add(o), dst.add(o), 4);
                    } else if a != 0 {
                        let inv = 255 - a;
                        *dst.add(o)     = (*src.add(o)     as u32 + (*dst.add(o)     as u32 * inv) / 255) as u8;
                        *dst.add(o + 1) = (*src.add(o + 1) as u32 + (*dst.add(o + 1) as u32 * inv) / 255) as u8;
                        *dst.add(o + 2) = (*src.add(o + 2) as u32 + (*dst.add(o + 2) as u32 * inv) / 255) as u8;
                    }
                    i += 1;
                }
            } else {
                // RGBA src → RGBX dst: identical byte order (alpha lands in the
                // ignored X slot). One row memcpy.
                core::ptr::copy_nonoverlapping(src, dst, vis_w * 4);
            }
            sy += 1;
        }
```

- [ ] **Step 3: aggiorna i costruttori di `WinDesc` in `wm.rs`**

(a) WIN_ARENA init (riga ~435):

```rust
static mut WIN_ARENA: [WinDesc; MAX_WINS] = [WinDesc {
    px: core::ptr::null(), px_len: 0, x: 0, y: 0, w: 0, h: 0, shadow: false,
    blend: false,
}; MAX_WINS];
```

(b) in `present()`: il Vec `descs` diventa a 8 campi
`(*const u8, usize, u32, u32, u32, u32, bool /*shadow*/, bool /*blend*/)` —
aggiorna la dichiarazione e i due `descs.push((px, len, ..., false))` /
`descs.push((px, len, x, y, w, h, true))` esistenti aggiungendo `false` come
ultimo elemento (blend), e il loop di copia nell'arena:

```rust
        for (i, (px, len, fx, fy, fw, fh, sh, bl)) in descs.iter().enumerate().take(n) {
            unsafe {
                WIN_ARENA[i] = WinDesc {
                    px: *px, px_len: *len, x: *fx, y: *fy, w: *fw, h: *fh,
                    shadow: *sh, blend: *bl,
                };
            }
        }
```

- [ ] **Step 4: verifica build**

Run: `WSL: make iso`
Expected: verde (blend mai attivo finché nessuna finestra overlay esiste).

---

### Task 2: finestra overlay kernel (flag, `wm.set_overlay`, compositing, spawn)

**Files:**
- Modify: `kernel/src/wasm/wt/wm.rs` — `WmState` (~693), costruzione WmState
  (cerca `WmState {`), `Window` (~1009+, ha già i campi v1), `spawn_named`
  literal (~1416), `add_to_linker` (~840, dopo set_background), run loop
  deferred requests (~2210, blocco bg_request), `present()` (~1690),
  `window_at` (~1180), snapshot taskbar (~1961), `Compositor::new` (~1178)

- [ ] **Step 1: campi nuovi**

In `WmState` (dopo `pub bg_request: bool`):

```rust
    /// Set dal guest via `wm.set_overlay()`; il run loop pinna QUESTA finestra
    /// come overlay notifiche (full-screen, z-TOP, alpha-blend) e poi lo azzera.
    pub overlay_request: bool,
    /// Cursore di lettura per-finestra sul kevent bus (`sys.events_poll`).
    pub kev_cursor: u64,
```

Trova la costruzione `WmState {` (grep `WmState {` in wm.rs, è nello spawn
path) e aggiungi all'init:

```rust
            overlay_request: false,
            kev_cursor: crate::kevent::current_seq(),
```

In `Window` (dopo `pub bg: bool`):

```rust
    /// Overlay notifiche: compositata per ULTIMA (z-top, sopra tutte), forzata
    /// full-screen, alpha-blend, esclusa da hit-test normale/taskbar/focus.
    /// Speculare a `bg`. Al reap si torna al fallback decor kernel (spec v2).
    pub overlay: bool,
```

e nel literal di `spawn_named` (dopo `bg: false,`): `overlay: false,`.

- [ ] **Step 2: host fn `wm.set_overlay`**

In `add_to_linker`, subito dopo la registrazione di `set_background` (~840):

```rust
    // wm.set_overlay(): flag THIS window as the notifications overlay
    // (full-screen, z-TOP, alpha-blended, input only on opaque pixels).
    // Deferred to the run loop, mirror of set_background. One overlay max.
    linker.func_wrap("wm", "set_overlay",
        |mut caller: Caller<'_, T>| { caller.data_mut().win().overlay_request = true; })?;
```

- [ ] **Step 3: applicazione deferred + helper**

Nel run loop, subito DOPO il blocco che applica `bg_request` (~2210):

```rust
            // 1b) Overlay requests: pin the requesting window as the notifications
            //     overlay (one max — extra requests are ignored with a warning).
            for i in 0..self.wins.len() {
                if self.wins[i].store.data().win.overlay_request {
                    self.wins[i].store.data_mut().win.overlay_request = false;
                    if self.overlay_index().is_some() {
                        crate::bwarn!("wm", "set_overlay: overlay already present, ignored win_id={}",
                                      self.wins[i].id);
                    } else {
                        self.wins[i].overlay = true;
                        self.dirty = true;
                        crate::binfo!("wm", "overlay window win_id={}", self.wins[i].id);
                    }
                }
            }
```

Helper accanto a `bg_index` (cerca `fn bg_index`):

```rust
    /// Indice della finestra overlay notifiche (None = fallback decor kernel).
    fn overlay_index(&self) -> Option<usize> {
        self.wins.iter().position(|w| w.overlay)
    }
```

- [ ] **Step 4: compositing — overlay per ultima, full-screen, blend**

In `present()`:

(a) dopo il blocco che forza il rect della bg (`if let Some(bi) = bg_idx { ... }`):

```rust
        // L'overlay notifiche è full-screen come la bg, ma in cima.
        let ov_idx = self.overlay_index();
        if let Some(oi) = ov_idx {
            self.wins[oi].rect = (0, 0, sw, sh);
        }
```

(b) nel loop dei descs salta l'overlay (`if ov_idx == Some(i) { continue; }`
accanto allo skip della bg) e DOPO il loop aggiungi:

```rust
        // Overlay notifiche: compositata per ULTIMA (sopra tutte), alpha-blend,
        // niente ombra, origine forzata (0,0).
        if let Some(oi) = ov_idx {
            if !self.wins[oi].minimized {
                if let Some((px, len, _, _, fw, fh)) = self.compose_window(oi) {
                    descs.push((px, len, 0, 0, fw, fh, false, true));
                }
            }
        }
```

- [ ] **Step 5: esclusioni input/taskbar/focus**

(a) `window_at` (~1180): aggiungi lo skip accanto a quello di `bg`/`minimized`:
`if w.overlay { continue; }` (adatta alla forma del loop esistente).

(b) snapshot taskbar (~1961): `if w.bg { continue; }` diventa
`if w.bg || w.overlay { continue; }`.

(c) cerca `fn focus_topmost_visible` e aggiungi lo skip dell'overlay accanto a
quello della bg (l'overlay non deve mai prendere focus).

- [ ] **Step 6: spawn di notify in `Compositor::new`**

Dopo il blocco `match initial { ... }` (~1178):

```rust
        // App overlay notifiche (egui): opzionale — se /bin/notify.cwasm manca
        // si resta sul fallback decor kernel (spec notifiche-egui-overlay §6).
        if let Some(m) = module_by_name("notify") {
            let _ = c.spawn_named("notify", m);
        }
```

(NB: notify chiama `wm.set_overlay()` al primo frame; fino ad allora è una
finestra normale per un giro di loop — innocuo, commit trasparente.)

- [ ] **Step 7: verifica build**

Run: `WSL: make iso && make run-test`
Expected: verdi (notify.cwasm non esiste ancora → fallback decor, nessun
cambiamento osservabile).

---

### Task 3: routing input per-pixel + gating del fallback decor

**Files:**
- Modify: `kernel/src/wasm/wt/wm.rs` — run loop input (~2090, blocco
  `while let Some(ev)`), `drain_kevents`, `tick_modal`, `draw_overlays`

- [ ] **Step 1: hit-test per-pixel sull'alpha**

Metodo nuovo accanto a `toast_at`:

```rust
    /// Hit-test per-pixel sull'overlay: true se (px,py) cade su un pixel della
    /// surface committata con alpha >= 32 (spec: soglia anti-ombra). La surface
    /// è full-screen, quindi coordinate schermo = coordinate surface.
    fn overlay_hit(&self, oi: usize, px: i32, py: i32) -> bool {
        if px < 0 || py < 0 { return false; }
        let s = self.wins[oi].store.data();
        let (w, h) = (s.win.win_w as i32, s.win.win_h as i32);
        if w == 0 || h == 0 || px >= w || py >= h { return false; }
        let o = (py as usize * w as usize + px as usize) * 4 + 3;
        s.win.pixels.get(o).map_or(false, |&a| a >= 32)
    }
```

- [ ] **Step 2: routing nel run loop**

Aggiungi `let mut overlay_btn = false;` accanto a `let mut btn_l = false;`
(~1754). Poi, dentro `while let Some(ev) = crate::gfx::pop()`, PRIMA dei
blocchi v1 `if self.modal.is_some()` / hit-test toast, inserisci:

```rust
                // Overlay notifiche viva: (a) modal grab — power pending ⇒ TUTTO
                // l'input all'overlay; (b) altrimenti hit-test per-pixel: solo i
                // pixel "dipinti" (alpha >= 32) catturano il mouse, il resto passa
                // alle finestre sotto. overlay_btn traccia la press consumata
                // dall'overlay così la release torna allo stesso owner (e una
                // release di un drag iniziato su una finestra NON viene rubata).
                if let Some(oi) = self.overlay_index() {
                    let grab = crate::power::pending().is_some();
                    let route = match ev.kind {
                        0 => grab,
                        1 => grab || overlay_btn || (!btn_l && self.overlay_hit(oi, cx, cy)),
                        2 if ev.p0 == 0 => {
                            let pressed = ev.p1 != 0;
                            if pressed { grab || (!btn_l && self.overlay_hit(oi, cx, cy)) }
                            else { grab || overlay_btn }
                        }
                        5 => grab || self.overlay_hit(oi, cx, cy),
                        _ => false,
                    };
                    if route {
                        match ev.kind {
                            0 => {
                                self.wins[oi].store.data_mut().win.events.push_back(ev);
                            }
                            1 => self.forward_mouse_move(oi, cx, cy),
                            2 => {
                                let pressed = ev.p1 != 0;
                                if pressed { overlay_btn = true; } else { overlay_btn = false; }
                                self.forward_left_button(oi, cx, cy, pressed);
                            }
                            5 => {
                                self.forward_mouse_move(oi, cx, cy);
                                self.wins[oi].store.data_mut().win.events.push_back(ev);
                            }
                            _ => {}
                        }
                        self.wins[oi].last_active_frame = self.frame_no;
                        continue;
                    }
                }
```

- [ ] **Step 3: gating del fallback in `drain_kevents`**

In testa a `drain_kevents`:

```rust
        // Overlay viva: gli eventi li legge LEI (sys.events_poll, suo cursore).
        // Qui solo: avanza il cursore kernel e sveglia l'overlay dormiente.
        if let Some(oi) = self.overlay_index() {
            let cur = crate::kevent::current_seq();
            if cur != self.kev_cursor {
                self.kev_cursor = cur;
                self.wins[oi].last_active_frame = self.frame_no;
            }
            return;
        }
```

- [ ] **Step 4: `tick_modal` riapre il modale in fallback**

Sostituisci l'inizio di `tick_modal`:

```rust
    fn tick_modal(&mut self) {
        // Overlay viva: il modale lo disegna lei — chiudi quello decor se aperto.
        if self.overlay_index().is_some() {
            if self.modal.take().is_some() { self.dirty = true; }
            return;
        }
        // Fallback: se c'è un pending e il modale decor non è aperto, aprilo
        // (copre anche il caso "overlay morta con countdown in corso").
        if self.modal.is_none() {
            if crate::power::pending().is_some() {
                self.modal = Some(PowerModal { last_secs: 0 });
                self.dirty = true;
            }
            return;
        }
        // ... resto invariato (match crate::power::pending() { ... })
    }
```

- [ ] **Step 5: `draw_overlays` decor solo in fallback**

In testa a `draw_overlays`:

```rust
        // Overlay egui viva → niente decor (toast/modale li disegna lei).
        if self.overlay_index().is_some() { return; }
```

- [ ] **Step 6: verifica build + regressione fallback**

Run: `WSL: make run-test CARGO_FEATURES=boot-checks`
Expected: PASS + `KEVENT_TEST: OK` (senza notify.cwasm il comportamento è
identico alla v1).

---

### Task 4: host fn `sys.events_poll` + `wm.power_pending` / `wm.power_cancel` (+ docs)

**Files:**
- Modify: `kernel/src/wasm/wt/wm.rs` `add_to_linker` (dopo `poll_event`, ~860)
- Modify: `docs/api/sys.md`, `docs/api/wm.md` (entry + Last reviewed)

- [ ] **Step 1: registra le tre host fn**

In `add_to_linker` (wm.rs), dopo il blocco `poll_event`:

```rust
    // sys.events_poll(buf_ptr) -> i32: un evento del kernel event bus per
    // chiamata, dal cursore di QUESTA finestra. 1 = record scritto (64 byte LE:
    // seq u64 | kind u16 | sev u8 | pad | ts u32 | payload 4xu32 | nome 32B
    // NUL-pad), 0 = niente di nuovo. Gap (eventi sovrascritti) → PRIMA un
    // record sintetico SUBSCRIBER_OVERFLOW{lost}, poi i reali alle chiamate
    // successive. Registrata qui (non in sys.rs) perché serve lo stato finestra.
    linker.func_wrap("sys", "events_poll",
        |mut caller: Caller<'_, T>, buf_ptr: i32| -> i32 {
            let cur = caller.data_mut().win().kev_cursor;
            let mut tmp = [crate::kevent::KEvent::ZERO; 1];
            let (n, lost) = crate::kevent::read_since(cur, &mut tmp);
            let mut rec = [0u8; 64];
            if lost > 0 {
                // Salta i persi: il prossimo poll parte dal primo disponibile.
                caller.data_mut().win().kev_cursor = cur + lost;
                rec[8..10].copy_from_slice(&crate::kevent::KIND_SUBSCRIBER_OVERFLOW.to_le_bytes());
                rec[10] = crate::kevent::SEV_INFO;
                rec[12..16].copy_from_slice(&(crate::timer::ticks() as u32).to_le_bytes());
                rec[16..20].copy_from_slice(&(lost as u32).to_le_bytes());
                rec[20..24].copy_from_slice(&((lost >> 32) as u32).to_le_bytes());
                crate::wasm::wt::mem::write(&mut caller, buf_ptr as u32, &rec);
                return 1;
            }
            if n == 0 { return 0; }
            let ev = tmp[0];
            caller.data_mut().win().kev_cursor = ev.seq;
            rec[0..8].copy_from_slice(&ev.seq.to_le_bytes());
            rec[8..10].copy_from_slice(&ev.kind.to_le_bytes());
            rec[10] = ev.severity;
            rec[12..16].copy_from_slice(&ev.ts_ticks.to_le_bytes());
            for (i, p) in ev.payload.iter().enumerate() {
                rec[16 + i * 4..20 + i * 4].copy_from_slice(&p.to_le_bytes());
            }
            if let Some(name) = crate::kevent::name_of(ev.seq) {
                let b = name.as_bytes();
                let l = b.len().min(32);
                rec[32..32 + l].copy_from_slice(&b[..l]);
            }
            crate::wasm::wt::mem::write(&mut caller, buf_ptr as u32, &rec);
            1
        })?;
    // wm.power_pending() -> i64: 0 = nessuna richiesta differita; altrimenti
    // (kind << 32) | tick_rimanenti, kind 1 = poweroff, 2 = reboot. Fonte di
    // verità del countdown del modale (l'app NON conta da sola).
    linker.func_wrap("wm", "power_pending",
        |_caller: Caller<'_, T>| -> i64 {
            match crate::power::pending() {
                None => 0,
                Some((kind, ticks)) => {
                    let k: i64 = match kind {
                        crate::power::PendingKind::Poweroff => 1,
                        crate::power::PendingKind::Reboot => 2,
                    };
                    (k << 32) | (ticks as u32 as i64)
                }
            }
        })?;
    // wm.power_cancel(): annulla la richiesta power differita (no-op se assente).
    linker.func_wrap("wm", "power_cancel",
        |_caller: Caller<'_, T>| { crate::power::cancel(); })?;
```

- [ ] **Step 2: docs (STESSO task)**

(a) `docs/api/sys.md`: nuova sezione (leggere il file per lo stile, poi):

```markdown
### `events_poll(buf_ptr) -> i32`
One kernel-event-bus record per call, from THIS window's private cursor.
Returns `1` (64-byte LE record written: seq u64 | kind u16 | severity u8 | pad
| ts_ticks u32 | payload 4×u32 | name 32B NUL-padded) or `0` (nothing new).
On reader overflow a synthetic `SUBSCRIBER_OVERFLOW` (kind 0x0001, payload =
lost lo/hi) is delivered first. Kinds/payloads: see
`docs/superpowers/specs/2026-06-11-kernel-event-bus-design.md` §2.
```

Aggiorna "Last reviewed" della pagina.

(b) `docs/api/wm.md`: dopo le entry poweroff/reboot:

```markdown
### `power_pending() -> i64`
`0` = no deferred request; else `(kind << 32) | ticks_remaining` (kind `1` =
poweroff, `2` = reboot; 100 ticks = 1 s). Source of truth for the countdown.

### `set_overlay()`
Flag THIS window as the notifications overlay: full-screen, composited ABOVE
all windows with per-pixel alpha, receives input only on pixels with
alpha ≥ 32 (plus ALL input while a power request is pending). One overlay max.

### `power_cancel()`
Cancel the pending deferred poweroff/reboot (no-op when none).
```

Aggiorna conteggio funzioni + "Last reviewed" (2026-06-11, +3 functions).

- [ ] **Step 3: verifica build**

Run: `WSL: make iso`
Expected: verde.

---

### Task 5: panic screen (`gfx::panic_screen`) + `kev-test panic`

**Files:**
- Modify: `kernel/src/gfx/mod.rs` (nuova fn in fondo al modulo)
- Modify: `kernel/src/main.rs:142-170` (panic handler)
- Modify: `kernel/src/wasm/host/proc.rs` (`ruos_kev_test` mode 4)
- Modify: `user/shell/src/main.rs` (`builtin_kev_test` modo "panic")
- Modify: `docs/api/ruos.md` (entry kev_test)

- [ ] **Step 1: `gfx::panic_screen`**

In fondo a `kernel/src/gfx/mod.rs` (usa gli atomics già presenti nel modulo:
`GFX_VIRT`, `GFX_PITCH`, `GFX_BPP`, `GFX_FMT`, `GFX_W`, `GFX_H`):

```rust
/// PANIC SCREEN — disegno DIRETTO sul framebuffer lineare dal panic handler.
/// Niente lock (i parametri fb sono atomics), niente alloc, IRQ già off: si può
/// chiamare da qualunque contesto morente, sia in GUI mode sia in console mode
/// (scrive sopra qualunque cosa). Solo 32 bpp; fb assente/bpp diverso = no-op.
/// `msg` = messaggio panic (con location), già formattato dal chiamante;
/// `footer` = riga finale (es. "reboot in 30 s" / "halted (panic-halt)").
pub fn panic_screen(msg: &str, footer: &str) {
    let base = GFX_VIRT.load(Ordering::Acquire);
    if base.is_null() { return; }
    let pitch = GFX_PITCH.load(Ordering::Acquire) as usize;
    let bpp = GFX_BPP.load(Ordering::Acquire);
    if bpp != 32 { return; }
    let bgr = GFX_FMT.load(Ordering::Acquire) == 1;
    let (sw, sh) = (GFX_W.load(Ordering::Acquire) as usize, GFX_H.load(Ordering::Acquire) as usize);
    if sw == 0 || sh == 0 { return; }

    // Sfondo rosso scuro pieno (#3a0d0d).
    let bg = if bgr { [0x0d, 0x0d, 0x3a, 0xff] } else { [0x3a, 0x0d, 0x0d, 0xff] };
    for y in 0..sh {
        let row = unsafe { base.add(y * pitch) };
        for x in 0..sw {
            unsafe { core::ptr::copy_nonoverlapping(bg.as_ptr(), row.add(x * 4), 4); }
        }
    }

    // Scrittura testo: font bitmap kernel, a capo automatico, bianco.
    let gw = crate::console::font::glyph_width();
    let gh = crate::console::font::glyph_height();
    let cols = (sw - 16) / gw;
    let mut cy = 16usize;
    let mut draw_line = |text: &str, cy: &mut usize| {
        if *cy + gh >= sh { return; }
        let mut cx = 16usize;
        for ch in text.chars() {
            if cx + gw >= sw { break; }
            let r = crate::console::font::raster_for_weight(ch, false);
            for (ry, rrow) in r.raster().iter().enumerate() {
                let py = *cy + ry;
                if py >= sh { break; }
                let row = unsafe { base.add(py * pitch) };
                for (rx, &a) in rrow.iter().enumerate() {
                    if a < 64 { continue; }
                    let px = cx + rx;
                    if px >= sw { continue; }
                    let v = a;
                    let p = if bgr { [v, v, v, 0xff] } else { [v, v, v, 0xff] };
                    unsafe { core::ptr::copy_nonoverlapping(p.as_ptr(), row.add(px * 4), 4); }
                }
            }
            cx += gw;
        }
        *cy += gh + 2;
    };

    draw_line("KERNEL PANIC", &mut cy);
    cy += gh / 2;
    // Messaggio (può essere multi-riga / lungo: spezza a `cols`).
    for line in msg.lines() {
        let mut rest = line;
        while !rest.is_empty() {
            let take = rest.char_indices().nth(cols).map_or(rest.len(), |(i, _)| i);
            draw_line(&rest[..take], &mut cy);
            rest = &rest[take..];
        }
    }
    cy += gh / 2;
    let mut hdr = crate::klog::Scratch::new();
    use core::fmt::Write as _;
    let _ = write!(hdr, "core={} tick={}", crate::cpu::cpu_id(), crate::timer::ticks());
    draw_line(core::str::from_utf8(hdr.as_bytes()).unwrap_or(""), &mut cy);
    cy += gh / 2;
    draw_line("--- klog tail ---", &mut cy);

    // Tail del klog ring: best-effort (try-path dentro klog::read è lock spin —
    // in panic gli altri sink hanno già fatto try_lock; qui accettiamo il rischio
    // residuo perché read è il consumo finale prima di reboot/halt).
    static mut TAIL: [u8; 4096] = [0; 4096];
    // SAFETY: panic path, single-flow (IRQ off su questo core); best-effort.
    let tail = unsafe { &mut *core::ptr::addr_of_mut!(TAIL) };
    let n = crate::klog::read(tail);
    let text = core::str::from_utf8(&tail[..n]).unwrap_or("");
    // Ultime ~15 righe.
    let lines: heapless::Vec<&str, 16> = {
        let mut v: heapless::Vec<&str, 16> = heapless::Vec::new();
        for l in text.lines().rev().take(15) {
            let _ = v.push(l);
        }
        v
    };
    for l in lines.iter().rev() {
        draw_line(l, &mut cy);
    }
    cy += gh / 2;
    draw_line(footer, &mut cy);
}
```

NB: verificare i nomi esatti degli atomics in testa a `gfx/mod.rs` (sono usati
da `blit`, riga ~126) e che `klog::Scratch` esponga `new`/`as_bytes` (sì:
`klog.rs:77+`, usati dal panic handler stesso).

- [ ] **Step 2: hook nel panic handler + reboot ritardato**

In `kernel/src/main.rs`, nel `#[panic_handler]`, DOPO il blocco framebuffer
console (riga ~152) e PRIMA del blocco exit-strategy, inserisci:

```rust
    // Panic screen: full-screen tecnico direttamente sul framebuffer (GUI o
    // console mode — stiamo morendo, si scrive sopra tutto). Best-effort.
    #[cfg(feature = "panic-halt")]
    let footer = "halted (panic-halt)";
    #[cfg(not(feature = "panic-halt"))]
    let footer = "reboot in 30 s";
    crate::gfx::panic_screen(core::str::from_utf8(msg).unwrap_or("KERNEL PANIC"), footer);
```

e sostituisci il blocco `#[cfg(not(feature = "panic-halt"))]` con:

```rust
    #[cfg(not(feature = "panic-halt"))]
    {
        // 30 s di schermo (busy-wait TSC: niente timer, IRQ off) poi reset
        // controllato — la macchina si riprende da sola, ma l'utente fa in
        // tempo a fotografare il panic screen.
        let tpm = crate::boot::clock::tsc_per_ms();
        if tpm > 0 {
            let end = crate::boot::clock::read_tsc().saturating_add(tpm.saturating_mul(30_000));
            while crate::boot::clock::read_tsc() < end {
                core::hint::spin_loop();
            }
        }
        crate::power::reboot();
        #[allow(unreachable_code)]
        loop { x86_64::instructions::hlt(); }
    }
```

- [ ] **Step 3: `kev-test panic` (mode 4)**

(a) `kernel/src/wasm/host/proc.rs`, in `ruos_kev_test` aggiungi il braccio
prima del default:

```rust
        4 => {
            // Debug del PANIC SCREEN: panica il kernel di proposito.
            panic!("kev-test: requested panic");
        }
```

(b) `user/shell/src/main.rs`, in `builtin_kev_test` aggiungi
`Some("panic") => 4,` dopo il braccio `Some("cancel") => 3,` e aggiorna il
messaggio d'errore e la riga di `builtin_help` in
`kev-test [toast|poweroff|reboot|cancel|panic]`.

(c) `docs/api/ruos.md`: estendi l'entry `kev_test` con
"`4` panica il kernel (debug del panic screen)". Aggiorna Last reviewed.

- [ ] **Step 4: test headless del panic**

Run (crea lo script e poi esegui — pattern del test negativo v1):

```bash
# build/panic-init.sh
echo panic-test: triggering kernel panic
kev-test panic
```

```bash
WSL: make iso INIT_SCRIPT=build/panic-init.sh ISO=build/os-panic.iso && \
  timeout 90 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom build/os-panic.iso \
    -serial stdio -display none -no-reboot -m 1024 > build/serial-panic.log 2>&1; \
  grep -F "KERNEL PANIC" build/serial-panic.log && echo PANIC_SEEN
```

Expected: `KERNEL PANIC: ... kev-test: requested panic` su seriale +
`PANIC_SEEN`; QEMU esce da solo dopo ~30 s (`-no-reboot` trasforma il reset in
exit). Verifica visiva dello SCHERMO in Task 10.

- [ ] **Step 5: verifica regressione**

Run: `WSL: make run-test`
Expected: PASS.

---

### Task 6: SDK — `gui-core::Renderer::set_clear` + API `ruos-window`

**Files (submodule `ruos-desktop` — niente commit):**
- Modify: `ruos-desktop/crates/gui-core/src/raster.rs` (~100, dopo Default)
- Modify: `ruos-desktop/crates/ruos-window/src/lib.rs` (extern ~20-57, wrapper ~160+, WindowState ~395)

- [ ] **Step 1: `set_clear` in gui-core**

In `raster.rs`, dentro `impl Renderer` (vicino a `render`):

```rust
    /// Imposta il colore di clear (sRGBA PREMOLTIPLICATO). [0,0,0,0] =
    /// trasparente — usato dall'overlay notifiche di ruos. Invalida canvas e
    /// diff (il prossimo render ridisegna tutto col nuovo clear).
    pub fn set_clear(&mut self, c: [u8; 4]) {
        self.clear = c;
        self.canvas = None;
        self.prev.clear();
    }
```

- [ ] **Step 2: extern + wrapper in ruos-window**

(a) nel `mod wm { extern "C" { ... } }` aggiungi:

```rust
        pub fn set_overlay(); // wm.set_overlay (flag THIS window as notifications overlay)
        pub fn power_pending() -> i64; // wm.power_pending → 0 | (kind<<32)|ticks
        pub fn power_cancel(); // wm.power_cancel (cancel deferred poweroff/reboot)
```

(b) nel `mod sys { extern "C" { ... } }` aggiungi:

```rust
        pub fn events_poll(ptr: *mut u8) -> i32; // 64-byte kernel-event record
```

(c) wrapper pubblici (dopo `set_background`):

```rust
/// Flag THIS window as the notifications overlay (full-screen, above all
/// windows, per-pixel alpha input). Deferred by the kernel to its run loop.
pub fn set_overlay() {
    unsafe { wm::set_overlay() }
}

/// Kind of a pending deferred power request.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PowerKind { Poweroff, Reboot }

/// The pending deferred power request: `(kind, ticks_remaining)` (100 ticks =
/// 1 s), or None. Source of truth for the notify app's countdown modal.
pub fn power_pending() -> Option<(PowerKind, u32)> {
    let r = unsafe { wm::power_pending() };
    if r == 0 { return None; }
    let kind = match (r >> 32) as u32 {
        1 => PowerKind::Poweroff,
        _ => PowerKind::Reboot,
    };
    Some((kind, (r & 0xffff_ffff) as u32))
}

/// Cancel the pending deferred poweroff/reboot (no-op when none).
pub fn power_cancel() {
    unsafe { wm::power_cancel() }
}

/// One kernel-event-bus record from `sys.events_poll` (see docs/api/sys.md).
pub struct KEventRec {
    pub seq: u64,
    pub kind: u16,
    pub severity: u8,
    pub ts_ticks: u32,
    pub payload: [u32; 4],
    pub name: String,
}

/// Poll THIS window's kernel-event cursor: one event per call, None when
/// drained. Overflow is delivered as a synthetic record (kind 0x0001).
pub fn events_poll() -> Option<KEventRec> {
    let mut b = [0u8; 64];
    if unsafe { sys::events_poll(b.as_mut_ptr()) } != 1 {
        return None;
    }
    let mut payload = [0u32; 4];
    for i in 0..4 {
        payload[i] = u32::from_le_bytes([b[16 + i * 4], b[17 + i * 4], b[18 + i * 4], b[19 + i * 4]]);
    }
    Some(KEventRec {
        seq: u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
        kind: u16::from_le_bytes([b[8], b[9]]),
        severity: b[10],
        ts_ticks: u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
        payload,
        name: nul_str(&b[32..64]),
    })
}
```

(d) costruttore overlay in `WindowState`:

```rust
    /// Come [`WindowState::new`] ma con canvas TRASPARENTE (clear [0,0,0,0]):
    /// per l'app overlay notifiche, che committa una surface full-screen dove
    /// solo i toast/modale sono dipinti. Usare con `frame_once_bare`.
    pub fn new_overlay() -> Self {
        let mut s = Self::new();
        s.renderer.set_clear([0, 0, 0, 0]);
        s
    }
```

- [ ] **Step 3: docs SDK**

`docs/api/ruos-window.md`: aggiungi alla tabella helper:

```markdown
| `set_overlay()` | Flag THIS window as the notifications overlay (full-screen, z-top, per-pixel alpha input). |
| `power_pending() -> Option<(PowerKind, u32)>` | Pending deferred power request (kind, ticks left). |
| `power_cancel()` | Cancel the pending deferred poweroff/reboot. |
| `events_poll() -> Option<KEventRec>` | One kernel-event-bus record per call (notify overlay). |
| `WindowState::new_overlay()` | WindowState with a TRANSPARENT canvas (overlay apps). |
```

Aggiorna "Last reviewed".

- [ ] **Step 4: verifica build PC (submodule)**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem/ruos-desktop && cargo check -p gui-core -p ruos-window --target wasm32-wasip1 2>&1 | tail -5'`
Expected: verde (warning ok).

---

### Task 7: app `notify` (`ruos-desktop/apps/notify-app`)

**Files (submodule):**
- Modify: `ruos-desktop/Cargo.toml:3-16` (members)
- Create: `ruos-desktop/apps/notify-app/Cargo.toml`
- Create: `ruos-desktop/apps/notify-app/src/lib.rs`

- [ ] **Step 1: workspace member**

In `ruos-desktop/Cargo.toml` aggiungi `"apps/notify-app",` dopo
`"apps/compositor-app",`.

- [ ] **Step 2: `Cargo.toml` della crate**

(copiare la forma di `apps/about-app/Cargo.toml` — leggere quel file — in
sostanza):

```toml
[package]
name = "notify-app"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
ruos-window = { path = "../../crates/ruos-window" }
gui-core = { path = "../../crates/gui-core" }
egui = { workspace = true }
```

- [ ] **Step 3: `src/lib.rs`**

```rust
//! `notify` — overlay notifiche del desktop (kevent bus → toast egui + modale
//! power). Finestra OVERLAY: full-screen trasparente, compositata sopra tutte
//! (kernel: alpha-blend + hit-test per-pixel), MAI nel launcher (niente
//! manifest). Spawnata dal compositor all'avvio se /bin/notify.cwasm esiste;
//! se muore, il kernel torna al fallback decor (spec notifiche-egui-overlay).

use ruos_window::{
    events_poll, frame_once_bare, power_cancel, power_pending, set_overlay,
    stay_awake, surface_size, wall_seconds, KEventRec, PowerKind, WindowState,
};

// Catalogo kind del kevent bus (spec 2026-06-11-kernel-event-bus-design §2).
const KIND_SUBSCRIBER_OVERFLOW: u16 = 0x0001;
const KIND_TEST: u16 = 0x0002;
const KIND_APP_CRASHED: u16 = 0x0201;
const KIND_APP_FUEL_EXHAUSTED: u16 = 0x0202;
const KIND_MEM_LOW: u16 = 0x0203;
// Power: gestiti via power_pending(), gli eventi 0x01xx non generano toast.
const KIND_SHUTDOWN_PENDING: u16 = 0x0101;
const KIND_REBOOT_PENDING: u16 = 0x0102;
const KIND_POWER_CANCELLED: u16 = 0x0103;

const SEV_WARN: u8 = 1;
const TOAST_LIFE_S: f64 = 5.0;
const TOAST_MAX_VISIBLE: usize = 3;

struct Toast {
    text: String,
    warn: bool,
    /// Quando è diventato visibile (None = ancora in coda FIFO).
    shown_at: Option<f64>,
}

static mut S: Option<WindowState> = None;
static mut INIT: bool = false;
static mut WH: (u32, u32) = (0, 0);
static mut TOASTS: Vec<Toast> = Vec::new();

fn toast_text(ev: &KEventRec) -> String {
    match ev.kind {
        KIND_SUBSCRIBER_OVERFLOW => {
            let lost = (ev.payload[0] as u64) | ((ev.payload[1] as u64) << 32);
            format!("bus eventi: persi {} eventi", lost)
        }
        KIND_APP_CRASHED => {
            let causa = match ev.payload[1] { 1 => "watchdog", 2 => "avvio fallito", _ => "crash" };
            if ev.name.is_empty() {
                format!("app win_id={} terminata ({})", ev.payload[0], causa)
            } else {
                format!("app '{}' terminata ({})", ev.name, causa)
            }
        }
        KIND_APP_FUEL_EXHAUSTED => {
            if ev.name.is_empty() {
                format!("pid {}: fuel esaurito", ev.payload[0])
            } else {
                format!("'{}' fermata: fuel esaurito", ev.name)
            }
        }
        KIND_MEM_LOW => format!(
            "memoria quasi esaurita: {}/{} frame liberi", ev.payload[0], ev.payload[1]),
        KIND_TEST => format!("evento di test ({})", if ev.name.is_empty() { "?" } else { &ev.name }),
        k => format!("kevent kind={:#06x}", k),
    }
}

#[no_mangle]
pub extern "C" fn frame() {
    // SAFETY: wasm single-thread; frame() è l'unico accessor, mai rientrante.
    #[allow(static_mut_refs)]
    unsafe {
        if !INIT {
            set_overlay();
            WH = surface_size();
            INIT = true;
        }
        if S.is_none() {
            S = Some(WindowState::new_overlay());
        }
        if WH.0 == 0 {
            WH = surface_size();
            return;
        }
        let (w, h) = WH;
        let s = S.as_mut().unwrap();
        let now = wall_seconds();

        // 1) Drena il bus → nuovi toast (gli eventi power non sono toast: il
        //    modale legge power_pending(), fonte di verità).
        while let Some(ev) = events_poll() {
            match ev.kind {
                KIND_SHUTDOWN_PENDING | KIND_REBOOT_PENDING | KIND_POWER_CANCELLED => {}
                _ => TOASTS.push(Toast {
                    text: toast_text(&ev),
                    warn: ev.severity >= SEV_WARN,
                    shown_at: None,
                }),
            }
        }
        // 2) Promozione FIFO + scadenza.
        let mut visible = 0usize;
        for t in TOASTS.iter_mut() {
            if visible >= TOAST_MAX_VISIBLE { break; }
            if t.shown_at.is_none() { t.shown_at = Some(now); }
            visible += 1;
        }
        TOASTS.retain(|t| match t.shown_at {
            Some(at) => now - at < TOAST_LIFE_S,
            None => true,
        });

        let pending = power_pending();
        let mut dismiss: Vec<usize> = Vec::new();
        let mut cancel = false;

        frame_once_bare(s, w, h, |ctx| {
            // Tema scuro coerente col desktop.
            ctx.set_visuals(egui::Visuals::dark());

            // -- toast: stack top-right, Frame arrotondato, fade sull'età --
            for (i, t) in TOASTS.iter().enumerate().take(TOAST_MAX_VISIBLE) {
                let age = now - t.shown_at.unwrap_or(now);
                // Fade-out nell'ultimo secondo di vita.
                let alpha = ((TOAST_LIFE_S - age).clamp(0.0, 1.0) * 255.0) as u8;
                let accent = if t.warn {
                    egui::Color32::from_rgba_unmultiplied(0xe0, 0xa0, 0x20, alpha)
                } else {
                    egui::Color32::from_rgba_unmultiplied(0x80, 0x80, 0x80, alpha)
                };
                let fill = egui::Color32::from_rgba_unmultiplied(0x20, 0x28, 0x30, alpha.saturating_sub(15));
                egui::Area::new(egui::Id::new(("toast", i)))
                    .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 44.0 + i as f32 * 64.0))
                    .show(ctx, |ui| {
                        let r = egui::Frame::window(&ctx.style())
                            .fill(fill)
                            .stroke(egui::Stroke::new(1.5, accent))
                            .corner_radius(8.0)
                            .show(ui, |ui| {
                                ui.set_max_width(280.0);
                                ui.label(
                                    egui::RichText::new(&t.text)
                                        .color(egui::Color32::from_rgba_unmultiplied(0xf0, 0xf0, 0xf0, alpha)),
                                );
                            });
                        if r.response.interact(egui::Sense::click()).clicked() {
                            dismiss.push(i);
                        }
                    });
            }

            // -- modale power: countdown da power_pending (fonte di verità) --
            if let Some((kind, ticks)) = pending {
                let secs = ticks / 100 + 1;
                let title = match kind {
                    PowerKind::Poweroff => "Spegnimento",
                    PowerKind::Reboot => "Riavvio",
                };
                egui::Window::new(title)
                    .collapsible(false)
                    .resizable(false)
                    .movable(false)
                    .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                    .show(ctx, |ui| {
                        ui.label(format!("tra {} s", secs));
                        ui.add_space(8.0);
                        if ui.button("Annulla").clicked() {
                            cancel = true;
                        }
                        ui.small("(Esc per annullare)");
                    });
                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
            }
        });

        if cancel {
            power_cancel();
        }
        for i in dismiss.into_iter().rev() {
            TOASTS.remove(i);
        }
        // Animazioni (fade/countdown) solo quando c'è qualcosa a schermo;
        // altrimenti dormi — il kernel ti sveglia al prossimo kevent.
        if !TOASTS.is_empty() || pending.is_some() {
            stay_awake();
        }
    }
}

#[no_mangle]
pub extern "C" fn _start() {}
```

NB egui 0.31: se `corner_radius` non esiste usare `rounding(8.0)`
(`egui::Frame` API — verificare contro le altre app del workspace).

- [ ] **Step 4: verifica build crate**

Run: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem/ruos-desktop && cargo build -p notify-app --target wasm32-wasip1 --release 2>&1 | tail -5'`
Expected: verde.

---

### Task 8: Makefile — `notify.cwasm` nell'ISO

**Files:**
- Modify: `Makefile:174-185` (regole app) e `:254-273` (iso deps + binstage)

- [ ] **Step 1: regola di build**

Dopo la regola `build/about.cwasm` (riga ~174), pattern identico:

```makefile
build/notify.cwasm: $(WT_PRECOMPILE) $(APP_SRCS) $(wildcard $(RUOS_DESKTOP)/apps/notify-app/src/*.rs $(RUOS_DESKTOP)/apps/notify-app/Cargo.toml)
	@mkdir -p build
	source $$HOME/.cargo/env && cd $(RUOS_DESKTOP) && cargo build -p notify-app --target wasm32-wasip1 --release
	$(WT_PRECOMPILE) $(RUOS_DESKTOP)/target/wasm32-wasip1/release/notify_app.wasm build/notify.cwasm
```

- [ ] **Step 2: wiring iso**

(a) riga `iso:` (~254): aggiungi `build/notify.cwasm` alle dipendenze (dopo
`build/notepad.cwasm`).
(b) blocco binstage (~269): aggiungi
`cp build/notify.cwasm build/binstage/notify.cwasm` dopo la riga di notepad.

- [ ] **Step 3: build ISO completa**

Run: `WSL: make iso`
Expected: verde, log contiene la compilazione di notify-app + wt-precompile di
notify.cwasm.

---

### Task 9: changelog + spec + verifica regressione

**Files:**
- Create: `CHANGELOG/<NN>-26-06-11-notifiche-egui-overlay.md` (NN = max+1 in CHANGELOG/, al momento della stesura 472)
- Modify: `docs/superpowers/specs/2026-06-11-notifiche-egui-overlay-design.md` (stato + nota §4)

- [ ] **Step 1: regressione completa**

Run: `WSL: make run-test && make run-test CARGO_FEATURES=boot-checks`
Expected: entrambi PASS (+ `KEVENT_TEST: OK` nel secondo).

- [ ] **Step 2: changelog**

Verifica il numero più alto in `CHANGELOG/` e crea l'entry (formato CLAUDE.md)
con: cosa (overlay alpha + sys.events_poll + power_pending/cancel +
set_overlay + app notify + panic screen + kev-test panic), perché (estetica
egui con garanzia fallback; FATAL kernel-side), file toccati (lista completa
da `git status`).

- [ ] **Step 3: spec**

- `Stato:` → `implementato — vedi CHANGELOG/<NN>-...`
- §4: sostituire la menzione `frame_once_overlay` con
  `WindowState::new_overlay()` + `frame_once_bare` (deviazione implementata).

---

### Task 10: verifica visiva (delegare all'utente se manca display)

- [ ] `make run` → `compositor`:
  1. `kev-test` (da SSH o terminal app) → toast egui arrotondato top-right
     con bordo ambra, fade-out ~5 s, click = dismiss;
  2. click su zone trasparenti sopra una finestra → la finestra risponde
     (hit-test per-pixel);
  3. bottone power → modale egui centrata, countdown a scalare, Annulla/Esc
     → annulla; lasciato scadere → spegnimento;
  4. `kev-test panic` → panic screen rosso full-screen con messaggio + klog
     tail; reboot automatico dopo ~30 s.
- [ ] Fallback: ISO senza notify (rinominare `build/binstage/notify.cwasm` e
  rigenerare, o `make iso` con la regola commentata) → toast/modale decor v1.

## Note di design (per l'esecutore)

- **`overlay_btn` vs `btn_l`**: la press consumata dall'overlay NON setta
  `btn_l`, così la release fuori dall'overlay non arriva alle finestre; una
  press partita su una finestra (`btn_l = true`) NON viene mai rubata
  dall'overlay nemmeno se la release cade su un toast (il drag delle finestre
  resta integro).
- **Cursore kernel vs cursore app**: due lettori indipendenti sul ring (è
  multi-reader by design). Con overlay viva il kernel avanza il proprio senza
  generare decor; l'app legge col suo via events_poll.
- **Eventi power non sono toast** nell'app: il modale si apre/chiude SOLO su
  `power_pending()` (fonte di verità) — niente stato duplicato.
- **Panic screen senza heap/lock**: tutto su atomics + buffer statici; il
  busy-wait è TSC (il timer è morto: IRQ off).
- **tiny-skia è premoltiplicato**: il blend kernel usa `src + dst*(1-a)`,
  NON `src*a + dst*(1-a)`.
