# Spec di design — Terminale reale integrato nel desktop egui (UI ↔ shell su PTY)

**Data:** 2026-06-07
**Topic:** `ui-terminal-integration`
**Stato:** proposta (spec → piano → implementazione)

> Repo: `ruos` (kernel) + submodule `ruos-desktop` (UI egui). Questa spec attraversa entrambe le sponde: il `DeskApp` portabile vive in `gui-core`, le host fn nel kernel.

---

## 1. Obiettivo + contesto

### Obiettivo

Sostituire il `Terminal` stub del desktop egui con un **terminale vero**: una finestra egui-native il cui contenuto è una **shell viva** (`/bin/shell.wasm`) attaccata a una coppia PTY del kernel, con rendering VT/ANSI completo (parser `vte`, character grid, glyph atlas TTF, dirty-cell blit) e input bidirezionale (tasti → byte/CSI → PTY).

### Cosa esiste oggi

- **PTY + shell nel kernel.** `kernel/src/pty/mod.rs` espone un pool statico di `NUM_PAIRS = 4` coppie master/slave. Una coppia si rivendica con `try_claim(idx) -> bool`, si rilascia con `release(idx)`. Input verso la shell: `master_input_push(idx, byte)` (passa per la line discipline `ldisc.rs` — ICRNL, ICANON, ECHO, ISIG/^C). Output dalla shell: `master_output_try(idx) -> Option<u8>` (non bloccante), `master_output_len(idx) -> usize`. Lifecycle: `is_claimed(idx)`, `request_shutdown(idx)` (SIGHUP), `is_shutdown(idx)`. Termios: `set_termios(idx, t)` / `termios_snapshot(idx) -> Termios` (struct 56 byte `repr(C)`).
- **Spawn della shell su una PTY.** `crate::wasm::ssh_spawn::spawn_shell_on_pty(idx)` → `crate::executor::enqueue_shell_pty(idx, "/bin/shell.wasm")` accoda un task dispatcher che istanzia il Fiber, ribinda lo stdio alla PTY e lancia la shell. La shell è **un proc normale** (instradato come ogni exec, **non** pinnato sul core della GUI).
- **SSH già fa esattamente questo bridge.** `kernel/src/ssh/sunset_io.rs`: `alloc_and_spawn_shell()` fa `try_claim(idx) && spawn_shell_on_pty(idx)`; il runner SSH legge il canale e fa `master_input_push(idx, b)` per byte; il bridge raccoglie l'output con un loop di `master_output_try(idx)` e lo scrive sul canale; alla morte della shell (`!is_claimed(idx) && master_output_len(idx) == 0`) chiude il canale e fa `request_shutdown(idx)`. **La finestra terminale riusa lo stesso pattern, sostituendo "canale SSH" con "finestra egui".**
- **ABI host wm/gfx senza accesso PTY.** Le app-finestra parlano il modulo host `wm` (`kernel/src/wasm/wt/wm.rs`, `add_to_linker`) e l'interfaccia WIT `ruos:gui` (`wit/ruos-gui.wit`): `commit`, `poll_event`, `app_id`, `spawn`, `window_size`, ecc. **Non c'è nessuna host fn per la PTY**: il terminale non ha modo, oggi, di leggere/scrivere una shell.
- **Lo stub Terminal.** `ruos-desktop/crates/gui-core/src/desktop/apps/terminal.rs`: `Terminal { history: Vec<String>, input: String }`, rende con `ui.monospace()` + `ui.text_edit_singleline()`, eco "command not found". Già impacchettato in una finestra: `ruos-desktop/apps/terminal-app/src/lib.rs` (cdylib reactor che chiama `frame_once(s, title, W, H, |ctx| CentralPanel...app.ui(ui))`).
- **Renderer CPU.** `gui-core` rasterizza con `tiny-skia` (rasterizzatore baricentrico scritto a mano, `raster.rs`), niente GPU. Le texture egui (font/color) passano per `TexturesDelta` e finiscono in `HashMap<TextureId, Pixmap>`. Il driver `Gui::frame` (`lib.rs`) fa il loop `poll_events → to_raw_input → ctx.run → tessellate → render → present` con dirty-rect.

### La regola d'oro (vincolo invalicabile)

`gui-core` deve restare **ruos-agnostico**: dipende SOLO da `egui` / `egui_extras` / `egui_demo_lib` / `epaint` / `tiny-skia` + il trait `Platform` + i tipi `abi`. **MAI** `winit`, `softbuffer`, host fn del kernel, `std::fs`-OS. Tutto l'I/O passa per `Platform`. Quindi il `Terminal` DeskApp **non può chiamare le host fn PTY direttamente**: deve ottenere i byte tramite una **nuova estensione del trait `Platform`**, implementata da due backend (ruos + PC). Questo è esattamente il punto dell'architettura a due sponde: il PC esercita lo stesso codice UI che girerà su ruos.

---

## 2. Architettura

### Layering

```
┌──────────────────────────────────────────────────────────────────────┐
│ gui-core (PORTABILE — egui/tiny-skia + Platform + abi, niente OS)      │
│   desktop/apps/terminal.rs   →  Terminal DeskApp                       │
│     ├─ term/vt.rs      vte::Parser + Perform → Grid (cols×rows)        │
│     ├─ term/grid.rs    Cell{ch, fg, bg, attrs} + cursor + scrollback   │
│     ├─ term/atlas.rs   glyph atlas TTF (rasterize-once) + dirty blit   │
│     └─ ui(): texture upload dirty bbox → ui.image(); keys → bytes      │
│   platform.rs   trait Platform { … + term_open/read/write/resize/close}│
└───────────────────────────────▲──────────────────────────────────────┘
                                 │ (trait Platform — unica cucitura)
        ┌────────────────────────┴───────────────────────────┐
        ▼                                                     ▼
┌──────────────────────────┐                  ┌──────────────────────────────┐
│ ruos-window SDK (ruos)    │                  │ pc-backend (PC, throwaway)    │
│  term_* → host fn `wm`/   │                  │  term_* → portable-pty +      │
│  `ruos:gui/term`          │                  │  shell host locale            │
└───────────▲──────────────┘                  └──────────────────────────────┘
            │ host fn (func_wrap su Linker<AppState>)
┌───────────┴──────────────────────────────────────────────────────────┐
│ kernel: wt/term.rs  term_open/read/write/resize/close                 │
│   wrappa  pty::{try_claim, master_output_try, master_input_push,      │
│           set_termios, request_shutdown, release} + spawn_shell_on_pty│
└───────────▲──────────────────────────────────────────────────────────┘
            │
   ┌────────┴─────────┐
   │ PTY pool (4) +   │   shell.wasm = proc normale (non pinnato GUI core)
   │ ldisc + shell    │
   └──────────────────┘
```

### Flusso per frame (output: shell → schermo)

```
shell scrive su slave PTY
  → master_output_try(idx) accumula byte           [kernel]
  → Platform::term_read(h, &mut buf) -> n           [drain non-bloccante]
  → for b in buf: vte::Parser.advance(&mut Perform, b)
  → Perform aggiorna Grid: print(ch)→Cell, csi_dispatch(SGR)→attrs/colori,
       execute(\n\r\b\t), cursor move, erase; segna celle dirty
  → atlas: per ogni cella dirty, blit glyph (cache per (ch, attrs)) nel
       pixel buffer guest RGBA8888, tintato fg/bg
  → texture upload del SOLO bbox dirty → egui TexturesDelta (partial pos)
  → ui.image(texture, size) dentro la CentralPanel della finestra
  → raster.rs (tiny-skia) → present/commit del dirty rect
```

### Flusso input (tasti → shell)

```
GfxEvent::Key{scancode,pressed}  (PS/2 set 1, egui RawInput)
  → mentre la finestra terminale ha il focus, il Terminal DeskApp NON usa
    egui::Event::Text per editing; intercetta i tasti e li traduce in byte:
       lettere/cifre/punteggiatura → byte UTF-8
       Enter → \r (0x0D); Backspace → 0x7F (VERASE); Tab → 0x09
       Ctrl-C → 0x03 (VINTR); Ctrl-D → 0x04 (VEOF); Esc → 0x1B
       frecce/Home/End → sequenze CSI: ↑=ESC[A ↓=ESC[B →=ESC[C ←=ESC[D
                                       Home=ESC[H End=ESC[F Del=ESC[3~
  → Platform::term_write(h, &bytes)
  → master_input_push(idx, b) per byte → ldisc (ICRNL/ICANON/ECHO/ISIG) → shell
```

### Flusso resize

```
la finestra cambia size → egui dà l'area disponibile in punti
  → cols = floor(avail_w / cell_w); rows = floor(avail_h / cell_h)
  → se (cols,rows) cambiano: Grid.reflow(cols,rows) + atlas pixbuf realloc
  → Platform::term_resize(h, cols, rows)
  → set_termios con winsize (campo aggiunto, vedi §3g) → la shell vede SIGWINCH/COLUMNS
```

---

## 3. Componenti dettagliati

### 3a. Parser vte + modello Grid + cursore + scrollback + attributi

`gui-core/src/desktop/apps/term/vt.rs` + `term/grid.rs`.

- **Parser.** Riusa `vte` (già nel kernel a `0.13`, `default-features=false`, `features=["no_std"]`; **pure Rust, nessuna dipendenza `std`/OS → compila `wasm32-wasip1` ed è dentro la regola d'oro**). Si istanzia `vte::Parser` e si implementa `vte::Perform` su un tipo `GridPerform<'a>` che possiede `&mut Grid`. I metodi rilevanti (modellati su `kernel/src/console/fb.rs:166-236`):
  - `print(&mut self, c: char)` → scrive `Cell` alla posizione del cursore, avanza cursore, wrap a fine riga.
  - `execute(&mut self, byte: u8)` → `\n` (line feed + scroll), `\r` (CR), `\x08` (backspace), `\t` (tab a colonne multiple di 8).
  - `csi_dispatch(params, intermediates, ignore, action)` → `m` (SGR: colori/attributi via `apply_sgr`, vedi sotto), `H`/`f` (cursor position), `A/B/C/D` (cursor up/down/fwd/back), `J` (erase display), `K` (erase line), `r` (scroll region — opzionale v1).
  - `csi_dispatch` e `esc_dispatch` non gestiti → no-op (graceful).
- **SGR.** Porta `kernel/src/console/ansi.rs:apply_sgr()` (16-color VGA, xterm-256, truecolor `38;2;r;g;b` / `48;2;r;g;b`). Output: `Color` = `[u8;3]` (RGB) per fg/bg + flag bold/underline/reverse.
- **Grid model.** `Cell { ch: char, fg: Color, bg: Color, attrs: Attrs }` (`Attrs`: bitflags bold/underline/reverse). `Grid { cols, rows, cells: Vec<Cell>, cursor: (col,row), cursor_visible: bool, dirty: Vec<(u16,u16)> per riga (min_col,max_col) }`. Il tracking dirty per-riga è identico a `kernel/src/console/grid.rs:22-34` (min_col/max_col per riga, reset dopo il flush).
- **Cursore.** Posizione `(col,row)`; reso come blocco/underline lampeggiante (XOR sul bg della cella, come `kernel/src/console/render.rs:tick_cursor`); il blink usa `Platform::wall_clock_secs()`. La cella sotto al cursore è sempre considerata dirty quando il cursore si muove o lampeggia.
- **Scroll.** Line feed a fondo schermo → scroll up: i ratei portati in `scrollback` (anello `VecDeque<Vec<Cell>>`, cap. `SCROLLBACK_LINES = 1000` v1). Lo scroll è un **memmove** delle righe (vedi 3b), non un re-render full.
- **Scrollback view.** v1: solo viewport live (no scroll-indietro interattivo con mouse wheel). Wheel/PageUp → fase 2 (vedi §7). Lo scrollback è comunque **memorizzato** così l'estensione è solo UI.

### 3b. Glyph atlas (rasterize-once, cache, tint-on-blit) + dirty-cell + scroll come memmove

`gui-core/src/desktop/apps/term/atlas.rs`.

- **Font.** Un monospace TTF embeddato (`include_bytes!`), rasterizzato con un font rasterizer puro-Rust. **Decisione:** riusare la **coverage atlas di epaint/egui** non è praticabile cella-per-cella (egui rasterizza in funzione del layout, non a griglia fissa). Si usa `ab_glyph` (puro Rust, `no_std`-capace, già nell'albero di dipendenze di epaint) per rasterizzare ogni glifo a una **alpha mask** una volta sola. Questo aggiunge `ab_glyph` a `gui-core` — **è puro Rust e graphics-only, non OS-specifico, quindi compatibile con la regola d'oro** (la regola vieta winit/softbuffer/OS, non un font rasterizer; va pinnato nel workspace `Cargo.toml`). In alternativa, riusare `noto-sans-mono-bitmap` (già usato dal kernel, bitmap pre-rasterizzato) — più semplice ma niente AA scalabile; vedi §7.
- **Atlas.** `GlyphCache: HashMap<(char, Attrs_subset), AlphaMask>` dove `AlphaMask { w, h, bearing, advance, coverage: Vec<u8> }`. `prewarm_ascii()` pre-carica 0x20..0x7E (come `kernel/src/console/glyphcache.rs:prewarm_ascii`). Cella size `cell_w × cell_h` derivata dalle metriche del font alla `font_px` scelta (default 16px).
- **Tint-on-blit.** L'alpha mask è monocromatica; al blit di una cella si compone `out = bg*(1-a) + fg*a` per pixel (`compose_cell`, identico concettualmente a `kernel/src/console/render.rs:10-65`). Il colore non è in atlas: l'atlas è solo coverage → un solo glifo serve a tutte le combinazioni fg/bg (cache piccola).
- **Pixel buffer guest.** Un `Vec<u8>` RGBA8888 di `cols*cell_w × rows*cell_h`, posseduto dal `Terminal`. Il blit tocca solo le celle dirty.
- **Scroll = memmove.** Su line feed che scrolla, invece di re-blittare ogni cella: `pixbuf.copy_within(riga_src.., dst)` di `(rows-1)` righe verso l'alto (memmove di `cell_h` scanline) + blit della sola riga nuova in fondo. Il bbox dirty per quel frame = l'intero pixbuf (scroll = full-height change) ma il **costo CPU** è un memmove + 1 riga di glifi, non `cols*rows` blit.
- **Dirty bbox.** Si aggrega l'unione delle celle dirty del frame in un rettangolo pixel (`DirtyRect { x,y,w,h }`, lo stesso tipo concettuale di `raster.rs:DirtyRect`). Solo quel sub-rettangolo viene caricato come delta di texture (vedi 3c).

### 3c. Integrazione egui (texture upload, ui.image, chrome, focus, tastiera, resize)

`gui-core/src/desktop/apps/terminal.rs` (riscritto).

- **Texture handle.** Il `Terminal` tiene un `Option<egui::TextureHandle>`. Alla prima `ui()`: `ctx.load_texture("term", ColorImage::from_rgba_unmultiplied([w,h], &pixbuf), TextureOptions::NEAREST)` (NEAREST = niente blur, pixel-perfect su monospace). Nei frame successivi, se c'è un dirty bbox: `tex.set_partial([x,y], ColorImage::from_rgba_unmultiplied([dw,dh], &sub), NEAREST)` — **carica solo il bbox dirty**, mappandosi su `set_texture` con `delta.pos = Some(...)` (`raster.rs:226-266` gestisce già i partial update).
- **Disegno.** `ui.image((tex.id(), egui::vec2(w as f32, h as f32)))` dentro la `CentralPanel` della finestra. La chrome (titlebar CSD, drag, resize, close) la fornisce `ruos-window`/egui — **il terminale non disegna la finestra, solo il suo contenuto**.
- **Focus.** Il pixbuf è disegnato con `ui.image`, che non è focusabile da solo; si avvolge in un `ui.interact(rect, id, Sense::click())` per catturare il focus al click e `ctx.memory_mut(|m| m.request_focus(id))`. Quando il terminale ha il focus, consuma gli eventi tastiera (non li lascia ad altri widget).
- **Tastiera → byte.** Dentro `ui()`, con focus, si legge `ui.input(|i| i.events.clone())` e si traduce **non in editing egui** ma in byte verso la shell:
  - `Event::Text(s)` → byte UTF-8 di `s` (copre lettere/cifre/punteggiatura, già filtrato da `input.rs` che esclude testo quando ctrl/alt premuti).
  - `Event::Key { key, pressed: true, modifiers, .. }` → mappa: `Enter→\r`, `Backspace→0x7F`, `Tab→\t`, `Escape→0x1B`, `ArrowUp→ESC[A`, `Down→ESC[B`, `Right→ESC[C`, `Left→ESC[D`, `Home→ESC[H`, `End→ESC[F`, `Delete→ESC[3~`. Con `modifiers.ctrl`: lettera `A..Z` → byte `0x01..0x1A` (Ctrl-C = `0x03`, Ctrl-D = `0x04`, Ctrl-L = `0x0C`...). Questa mappa sfrutta il fatto che `input.rs` già produce `egui::Key::C` + `modifiers.ctrl` (non `Event::Text("c")`) quando ctrl è premuto.
  - I byte raccolti nel frame → un solo `Platform::term_write(handle, &bytes)`.
- **Resize → cols/rows.** Dall'area disponibile in egui (`ui.available_size()` in punti, `ppp=1` su ruos/PC): `cols = (avail.x / cell_w).floor()`, `rows = (avail.y / cell_h).floor()`. Se cambiano rispetto al frame scorso: `grid.reflow(cols,rows)`, realloc pixbuf, ricrea/resize la texture e chiama `Platform::term_resize(handle, cols, rows)`.
- **Apertura/chiusura.** Alla prima `ui()` il `Terminal` chiama `Platform::term_open()` (lazy) e tiene il `TermHandle`. Su drop/chiusura finestra → `Platform::term_close(handle)`. La rilevazione "shell morta" (EOF) chiude la finestra (o mostra "[process exited]"): `term_read` ritorna un sentinel `-1` (vedi 3d).

> **Nota di accesso a `Platform` dal DeskApp.** Oggi `DeskApp::ui(&mut self, ui: &mut egui::Ui)` non riceve `Platform`. Estensione minima: il `Terminal` ottiene i byte non dentro `ui()` ma in un **pump per-frame** guidato dal driver. Si aggiunge al trait `DeskApp` un metodo opzionale `fn pump(&mut self, _p: &mut dyn TermIo) {}` chiamato da `Gui::frame`/`frame_once` prima di `ui()`, dove `TermIo` è l'oggetto-capability minimale (sotto-trait di `Platform` con solo i `term_*`). `ui()` resta puramente di disegno (legge il pixbuf già aggiornato). Questo tiene `DeskApp::ui` invariato per gli altri app e dà al terminale il canale I/O senza rompere la regola d'oro (passa un `&mut dyn TermIo`, non host fn). In `terminal-app`, il reactor chiama `app.pump(sdk_as_termio)` poi `frame_once(...)`.

### 3d. Estensione del trait `Platform` (metodi e tipi esatti)

`gui-core/src/platform.rs`. Si aggiunge un blocco PTY al trait (con default no-op così PC e ruos restano compilabili anche prima di implementarlo, e i backend che non vogliono terminali non sono forzati):

```rust
/// Handle opaco a una sessione terminale (coppia PTY + shell). 0..NUM_PAIRS-1
/// su ruos; indice locale su PC. -1 = nessun handle.
pub type TermHandle = i32;

pub trait Platform {
    // ... metodi esistenti: surface_info, poll_events, present, wall_clock_secs, poweroff ...

    /// Apre una sessione: rivendica una coppia PTY e ci lancia la shell.
    /// Ritorna l'handle, o `None` se nessuna coppia è libera (max raggiunto).
    fn term_open(&mut self) -> Option<TermHandle> { None }

    /// Drena fino a `buf.len()` byte di output della shell (NON bloccante).
    /// Ritorna il numero di byte scritti in `buf` (0 = niente pronto ora),
    /// oppure `-1` se la shell è terminata (EOF: handle da chiudere).
    fn term_read(&mut self, h: TermHandle, buf: &mut [u8]) -> i32 { -1 }

    /// Invia byte all'input della shell (passano per la line discipline).
    fn term_write(&mut self, h: TermHandle, bytes: &[u8]) { let _ = (h, bytes); }

    /// Comunica la nuova geometria (winsize) alla sessione.
    fn term_resize(&mut self, h: TermHandle, cols: u16, rows: u16) { let _ = (h, cols, rows); }

    /// Chiude la sessione: SIGHUP alla shell + rilascia la coppia PTY.
    fn term_close(&mut self, h: TermHandle) { let _ = h; }
}
```

`TermIo` (il sotto-trait passato a `DeskApp::pump`) ri-espone solo questi cinque metodi; `Platform` lo implementa banalmente. Tipi puri (`i32`, `u16`, `&[u8]`) → nessuna dipendenza OS, regola d'oro rispettata.

### 3e. Implementazione ruos-backend (`ruos-window` SDK → host fn)

`ruos-desktop/crates/ruos-window/src/lib.rs`. Si aggiunge il modulo host `term` accanto a `wm` (stesso pattern `#[link(wasm_import_module = "...")]`):

```rust
mod term {
    #[link(wasm_import_module = "term")]
    extern "C" {
        pub fn open() -> i32;                                   // -> handle | -1
        pub fn read(h: i32, ptr: *mut u8, cap: u32) -> i32;     // -> n | -1 (EOF)
        pub fn write(h: i32, ptr: *const u8, len: u32);
        pub fn resize(h: i32, cols: u32, rows: u32);
        pub fn close(h: i32);
    }
}
```

Il `WindowState` (o un nuovo `RuosTermIo`) implementa `Platform::term_*` chiamando queste. `term_read` passa `buf.as_mut_ptr()` + `buf.len()`; il kernel scrive nei byte guest via `mem::write` e ritorna il count. Nessun cambiamento al loop `frame_once`: il reactor di `terminal-app` chiama `app.pump(...)` prima di `frame_once`.

### 3f. Implementazione pc-backend (→ PTY locale per lo sviluppo)

`ruos-desktop/backends/pc-backend/src/main.rs`. `PcPlatform` implementa i `term_*` con una **PTY host** via il crate `portable-pty` (cross-platform, su Windows usa ConPTY): `term_open` fa `PtySystem::openpty()` + `CommandBuilder::new("cmd"/"bash"/"sh")` → `slave.spawn_command()`, tiene `Box<dyn MasterPty>` + il `reader`/`writer` in un `Vec<PcTermSession>` indicizzato dall'handle. `term_read` legge non-bloccante dal master (reader in un thread con canale `mpsc`, drain del canale qui). `term_write` scrive sul master writer. `term_resize` chiama `master.resize(PtySize{rows,cols,..})`. `term_close` droppa la sessione (chiude il master → la shell riceve HUP). `portable-pty` sta SOLO in `pc-backend` (throwaway) — **non** in `gui-core`. Questo dà parità di sviluppo: una shell host vera in una finestra winit, stesso codice UI di ruos.

### 3g. Host fn kernel + interfaccia WIT (wrap dell'API PTY esistente)

**Nuovo file `kernel/src/wasm/wt/term.rs`** con `pub fn add_to_linker<T: HasWindow + 'static>(linker: &mut Linker<T>)`, registrato accanto a `wm::add_to_linker` dove si costruisce il `Linker<AppState>` per le finestre. Pattern `func_wrap` identico a `wm.rs` (accesso memoria guest via `crate::wasm::wt::mem::read/write`). Per il mapping handle→PTY si tiene una piccola tabella nello stato della finestra (o uno stato globale `IrqMutex<[Option<TermSlot>; NUM_PAIRS]>` se l'handle è semplicemente l'`idx` della coppia — più semplice e sufficiente):

```rust
// term.open() -> i32: rivendica una coppia PTY libera, ci lancia la shell.
linker.func_wrap("term", "open", |_caller: Caller<'_, T>| -> i32 {
    for idx in 0..crate::pty::NUM_PAIRS {
        if crate::pty::try_claim(idx) {
            crate::wasm::ssh_spawn::spawn_shell_on_pty(idx); // enqueue_shell_pty(idx, "/bin/shell.wasm")
            return idx as i32;
        }
    }
    -1 // nessuna coppia libera (max NUM_PAIRS terminali concorrenti)
})?;

// term.read(h, ptr, cap) -> i32: drena master output (non bloccante).
// -1 se la shell è morta e il buffer è vuoto (EOF), come fa il bridge SSH.
linker.func_wrap("term", "read", |mut caller: Caller<'_, T>, h: i32, ptr: i32, cap: i32| -> i32 {
    let idx = h as usize;
    if idx >= crate::pty::NUM_PAIRS { return -1; }
    if !crate::pty::is_claimed(idx) && crate::pty::master_output_len(idx) == 0 {
        return -1; // EOF — vedi sunset_io.rs:348-349
    }
    let mut out: Vec<u8> = Vec::new();
    let want = (cap.max(0)) as usize;
    while out.len() < want {
        match crate::pty::master_output_try(idx) {  // non bloccante
            Some(b) => out.push(b),
            None => break,
        }
    }
    if out.is_empty() { return 0; }
    crate::wasm::wt::mem::write(&mut caller, ptr as u32, &out);
    out.len() as i32
})?;

// term.write(h, ptr, len): byte dell'utente → input shell (passa per ldisc).
linker.func_wrap("term", "write", |mut caller: Caller<'_, T>, h: i32, ptr: i32, len: i32| {
    let idx = h as usize;
    if idx >= crate::pty::NUM_PAIRS || !crate::pty::is_claimed(idx) { return; }
    if let Some(b) = crate::wasm::wt::mem::read(&mut caller, ptr as u32, len as u32) {
        for &byte in &b { crate::pty::master_input_push(idx, byte); } // come sunset_io.rs:289-298
    }
})?;

// term.resize(h, cols, rows): aggiorna winsize nei termios.
linker.func_wrap("term", "resize", |_caller: Caller<'_, T>, h: i32, cols: i32, rows: i32| {
    let idx = h as usize;
    if idx >= crate::pty::NUM_PAIRS { return; }
    let mut t = crate::pty::termios_snapshot(idx);
    // winsize: richiede aggiungere i campi ws_col/ws_row (vedi nota sotto).
    t.set_winsize(cols as u16, rows as u16);
    crate::pty::set_termios(idx, t);
})?;

// term.close(h): SIGHUP alla shell + rilascia la coppia.
linker.func_wrap("term", "close", |_caller: Caller<'_, T>, h: i32| {
    let idx = h as usize;
    if idx >= crate::pty::NUM_PAIRS { return; }
    crate::pty::request_shutdown(idx); // SIGHUP, come sunset_io.rs:396
    crate::pty::release(idx);          // libera la coppia per riuso
})?;
```

**Winsize.** Il codebase attuale **non espone** `winsize`/`TIOCGWINSZ` (verificato: nessun riferimento in `kernel/src/pty/`). Va aggiunto: due `u16` `ws_col`/`ws_row` nella struct `PtyPair` (o in `termios.rs`) + un getter che la shell legge a `init`/su SIGWINCH. v1 minimo: `term_resize` aggiorna i campi; la shell li legge per dimensionare l'editing (se la shell non li consuma ancora, `resize` è no-op innocuo e si attiva quando la shell li userà). Questo è l'unico tassello kernel **nuovo** oltre alle host fn (tutto il resto è wrap).

**WIT — nuova interfaccia `ruos:gui/term`** (`wit/ruos-gui.wit`), aggiunta al `world`:

```wit
interface term {
  // -1 = nessuna coppia libera; altrimenti handle (idx PTY).
  open: func() -> s32;
  // ritorna i byte letti; lista vuota = niente pronto; assenza segnalata via len.
  read: func(handle: s32, cap: u32) -> list<u8>;
  write: func(handle: s32, bytes: list<u8>);
  resize: func(handle: s32, cols: u32, rows: u32);
  close: func(handle: s32);
}
world ruos-gui {
  import gfx;
  import power;
  import term;   // NUOVO
}
```

(Le finestre ruos linkano oggi con il modulo `wm` raw, non con la lowering WIT completa; la WIT resta la spec dell'ABI e documenta il contratto. L'implementazione concreta segue il pattern `wm` raw — `read` ritorna il count come `i32` scrivendo nel buffer guest, più efficiente del `list<u8>` allocante; la WIT descrive la semantica, vedi nota in `wm.rs:533-534` sullo stesso disallineamento WIT vs raw già presente per `poll_event`.)

---

## 4. Performance

**Perché atlas + dirty-blit è veloce sul renderer CPU/tiny-skia.**

- **Caso tipico: poche celle/frame.** Digitando, una shell aggiorna l'eco di 1 carattere + sposta il cursore: ~2-3 celle dirty. Output di `ls`: una manciata di righe, ma solo all'arrivo dei byte; nei frame intermedi il pixbuf è statico → **zero blit, zero upload texture, dirty bbox vuoto** (il driver `Gui::frame` già esce con damage vuoto quando nulla cambia, come fa col wallpaper statico, `raster.rs:143-173`). 60fps banale: la maggior parte dei frame non fa nulla.
- **Costo full-repaint.** Riempire l'intera griglia (es. `clear` + un TUI tipo htop che ridisegna tutto): `cols*rows` blit di glifo. A 80×25 = 2000 celle; ogni blit è `cell_w*cell_h` (~8×16 = 128) compose con alpha → ~256K operazioni pixel. Trascurabile (< 1ms) e accade solo nel frame in cui il TUI ridisegna.
- **Scroll = memmove, non re-render.** Scroll di una riga = `copy_within` di `(rows-1)*cell_h` scanline (un memmove contiguo, bandwidth-bound, microsecondi) + 1 riga di glifi. È l'operazione più frequente in un terminale ed è quasi gratis. (Stessa logica del kernel console, ma qui in un buffer privato.)
- **Upload texture = solo bbox dirty.** `set_partial` carica `dw*dh` byte, non l'intera texture. Per typing, il bbox è ~poche celle → KB. La pipeline egui poi tessella **una `Mesh` con 2 triangoli** (l'`ui.image`) che tiny-skia rasterizza con UV bilineari (`raster.rs:420-550`) — costo proporzionale all'area della finestra disegnata, non al numero di celle.
- **`vte` è O(byte).** Il parser è una macchina a stati pura, costo lineare nei byte ricevuti; nessuna allocazione nel path comune (`print`).
- **Target: 60fps.** Frame statici escono in ~0 (damage vuoto). Frame con typing: 1 memmove + ~3 blit + ~KB upload + 1 quad → ben sotto 16ms. Frame TUI full-repaint: una manciata di ms, isolati. Su ruos la shell è un proc non pinnato → non blocca il compositor; il drain è non-bloccante (`master_output_try`), quindi anche con shell sotto carico la GUI resta a 60fps.

---

## 5. File toccati

**Kernel (`ruos`):**
- `kernel/src/wasm/wt/term.rs` — **nuovo**: `add_to_linker` con `term.{open,read,write,resize,close}` (wrap di `pty::*` + `spawn_shell_on_pty`).
- `kernel/src/wasm/wt/mod.rs` (o dove si costruisce `Linker<AppState>` per le finestre) — chiamare `term::add_to_linker(linker)` accanto a `wm::add_to_linker`.
- `kernel/src/pty/mod.rs` + `kernel/src/pty/termios.rs` — **nuovo**: campi `winsize` (`ws_col`/`ws_row`) + setter/getter; `set_winsize` su `Termios` o `PtyPair`. Niente altro: `try_claim/release/master_*_*/request_shutdown/is_claimed/set_termios/termios_snapshot` esistono già.
- `wit/ruos-gui.wit` — aggiungere `interface term { ... }` + `import term;` nel `world ruos-gui`.

**gui-core (`ruos-desktop`, PORTABILE):**
- `crates/gui-core/src/platform.rs` — aggiungere `TermHandle` + i 5 metodi `term_*` (default no-op) al trait `Platform`; definire il sotto-trait `TermIo`.
- `crates/gui-core/src/desktop/app_trait.rs` — aggiungere `fn pump(&mut self, _io: &mut dyn TermIo) {}` (default no-op) al trait `DeskApp`.
- `crates/gui-core/src/desktop/apps/term/mod.rs` — **nuovo** modulo.
- `crates/gui-core/src/desktop/apps/term/vt.rs` — **nuovo**: `GridPerform` (`impl vte::Perform`) + porting `apply_sgr`.
- `crates/gui-core/src/desktop/apps/term/grid.rs` — **nuovo**: `Cell`, `Attrs`, `Color`, `Grid`, cursore, scrollback, dirty per-riga.
- `crates/gui-core/src/desktop/apps/term/atlas.rs` — **nuovo**: `GlyphCache` (ab_glyph), `AlphaMask`, `compose_cell`/blit, scroll-memmove, dirty bbox.
- `crates/gui-core/src/desktop/apps/terminal.rs` — **riscritto**: `Terminal` con `Option<TermHandle>`, pixbuf RGBA, `TextureHandle`, `pump()` (drain `term_read` → `vte` → grid → atlas → pixbuf + tasti → `term_write`) e `ui()` (texture upload dirty + `ui.image` + focus/resize → `term_resize`).
- `crates/gui-core/Cargo.toml` + workspace `Cargo.toml` — aggiungere `vte` (`no_std`) e `ab_glyph` pinnati; il TTF embeddato.
- `crates/gui-core/src/lib.rs` (`Gui::frame`) — chiamare `app.pump(platform)` prima del disegno (per il path PC/driver generico).

**Backend ruos (`ruos-desktop`):**
- `crates/ruos-window/src/lib.rs` — **nuovo** modulo `mod term` (`#[link(wasm_import_module="term")]`) + impl `Platform::term_*` (o `TermIo`); `frame_once` invariato.
- `apps/terminal-app/src/lib.rs` — chiamare `app.pump(sdk_termio)` prima di `frame_once`.

**Backend PC (`ruos-desktop`, throwaway):**
- `backends/pc-backend/src/main.rs` — `PcTermSession` + impl `Platform::term_*` via `portable-pty`.
- `backends/pc-backend/Cargo.toml` — aggiungere `portable-pty`.

---

## 6. Test / accettazione

**PC-side prima (via `pc-backend`, ciclo di sviluppo veloce):**
1. `cargo run -p pc-backend` → aprire la finestra Terminal dal launcher → compare un **prompt di shell host vera** (`bash`/`cmd`).
2. `ls -la` → output con colori (se la shell li emette) reso da `vte`+atlas; scroll corretto.
3. Un TUI tipo `htop`/`top`/`vim` (o `htop`-like) → ridisegno full-screen corretto, cursore alla posizione giusta, colori/attributi (bold/reverse).
4. **Resize** della finestra → cols/rows cambiano, `term_resize` chiamato, il TUI si riadatta (la shell vede COLUMNS/LINES).
5. **Ctrl-C** durante un `sleep 100` / loop → interrompe (byte `0x03` → ISIG → SIGINT). **Ctrl-D** su prompt vuoto → EOF/exit. Frecce/Home/End in `bash` (line editing) → muovono il cursore (CSI corretti).
6. Chiusura finestra → `term_close` → la shell host riceve HUP e termina (nessun processo orfano).

**Test unitari `gui-core` (`cargo test -p gui-core`):**
- `vt.rs`: feed di sequenze ANSI note → asserzioni su `Grid` (es. `ESC[31mX` → cella `X` fg rosso; `\n` a fondo → scroll; `ESC[2J` → clear).
- `atlas.rs`: blit di una cella → pixel attesi (glifo noto, fg/bg noti); scroll-memmove → confronto pixbuf.
- mapping tasti→byte: `Ctrl-C`→`[0x03]`, `ArrowUp`→`ESC[A`, `Enter`→`[0x0D]`.

**ruos-side (sul kernel reale, dopo PC):**
7. Boot GUI → launcher → Terminal → compare il **prompt della shell ruos** (`/bin/shell.wasm`) su una coppia PTY.
8. `ps` (o equivalente) nella shell → mostra il proc shell (non pinnato sul core GUI; il compositor resta fluido mentre la shell lavora).
9. **Più finestre terminale** → più coppie PTY (fino a `NUM_PAIRS = 4`); la quinta `term_open` ritorna `-1` → la UI mostra "no free terminal" (graceful).
10. Output sostenuto (es. `cat` di un file lungo) → la GUI resta a 60fps (drain non-bloccante), nessun freeze del compositor; chiusura finestra → coppia rilasciata e riusabile.

---

## 7. Rischi + alternative considerate

### Alternative al renderer (RIGETTATE)

- **(a) egui-widget-per-cella** (un `Label`/`RichText` per carattere, o `LayoutJob` per riga). Re-tessella ogni frame: migliaia di glifi → migliaia di quad nel tessellatore + nel rasterizzatore baricentrico tiny-skia ad ogni frame, anche quando nulla cambia. **Troppo lento su CPU**; perde il dirty-blit. Rigettata.
- **(b) blit della console bitmap del kernel** (`noto-sans-mono-bitmap`, path `kernel/src/console`). Veloce e già esiste, ma **non moderno**: niente TTF/AA scalabile, niente selezione/scrollback ergonomici, font fisso. Mantenuta come **fallback** se `ab_glyph` desse problemi di footprint su `wasm32-wasip1` (vedi sotto), ma non la scelta primaria.

La scelta è la tecnica **alacritty/wezterm** adattata a CPU/tiny-skia: parser `vte` → grid → glyph atlas (rasterize-once) → blit dirty-cell → texture egui → `ui.image`.

### Rischi

- **`vte` su wasm.** `vte 0.13` è pure-Rust, `no_std`, zero dipendenze OS → compila `wasm32-wasip1` (già nel kernel). Rischio basso. Va pinnato alla stessa versione del kernel per coerenza.
- **`ab_glyph` in `gui-core`.** Aggiunge una dipendenza graphics-only; è pure-Rust e già transitivamente presente via epaint, quindi non viola la regola d'oro né rompe il porting. Rischio: footprint/`include_bytes!` del TTF. Mitigazione: prewarm ASCII + cache; fallback (b) se serve.
- **Perf CPU full-repaint.** TUI che ridisegnano tutto ogni frame (rari) costano `cols*rows` blit. Mitigazione: dirty-cell tracking lo limita ai frame realmente cambiati; il typing/scroll comune è economico. Target 60fps tenuto.
- **I/O cooperativo non-bloccante.** `term_read` DEVE essere non-bloccante (`master_output_try`, non `master_output_read` async che bloccherebbe): il compositor è single-thread cooperativo, un read bloccante lo freezerebbe. Il drain è bounded da `cap` per frame (back-pressure naturale: l'output in eccesso resta nel buffer master e si drena nei frame seguenti).
- **`NUM_PAIRS = 4` = max terminali concorrenti** (condivisi con SSH: ogni sessione SSH e ogni finestra terminale consumano una coppia). `term_open` oltre il limite ritorna `-1`; la UI lo gestisce. Allargare il pool è un cambiamento kernel separato.
- **Selezione + scrollback interattivo.** Fuori scope v1 (lo scrollback è *memorizzato* ma non navigabile da UI; nessuna selezione/copia col mouse). Fase 2: wheel/PageUp per scorrere il `VecDeque` di scrollback; drag-select → copia. L'architettura (grid + scrollback già in memoria) li abilita senza riscrivere il renderer.
- **Winsize è codice kernel nuovo.** Unico tassello non-wrap; se la shell non consuma ancora ws_col/ws_row, `term_resize` è innocuo (no-op funzionale) finché la shell non lo legge.
- **Race claim PTY.** `try_claim` è già atomico (usato da SSH); il loop in `term.open` itera finché ne trova una libera. Nessuna race nuova.

---

## 8. Milestone (incrementali)

1. **Vendor `vte` + modello Grid.** Aggiungere `vte`/`ab_glyph`/TTF al workspace; `grid.rs` (Cell/Attrs/Color/Grid/cursor/scrollback/dirty) + `vt.rs` (`GridPerform` + `apply_sgr`). Test unitari ANSI→Grid. (Nessuna UI ancora.)
2. **Atlas renderer.** `atlas.rs`: `GlyphCache` rasterize-once, `compose_cell`/blit tintato, scroll-memmove, dirty bbox → pixbuf RGBA. Test pixel (glifo + scroll).
3. **Estensione `Platform` + pc-backend (sviluppo su PC).** Aggiungere `TermHandle`/`term_*` + `TermIo` al trait e `DeskApp::pump`; implementare `term_*` in `pc-backend` via `portable-pty`. Riscrivere `terminal.rs` (pump+ui+texture+input+resize). **Esito intermedio: shell host vera in finestra winit** — il path UI completo è esercitato e debuggato su PC.
4. **Host fn kernel + ruos backend.** `kernel/src/wasm/wt/term.rs` (`open/read/write/resize/close` su `pty::*` + `spawn_shell_on_pty`), winsize in `pty`, registrazione nel `Linker<AppState>`, WIT `interface term`; `mod term` in `ruos-window` + impl `Platform::term_*`.
5. **Wire del DeskApp + launcher.** `terminal-app` chiama `app.pump(sdk_termio)` prima di `frame_once`; verificare lifecycle (open lazy, close su chiusura finestra, EOF su shell morta). Test ruos-side: prompt shell reale, `ps`, finestre multiple = coppie PTY multiple, 60fps sotto output sostenuto.
