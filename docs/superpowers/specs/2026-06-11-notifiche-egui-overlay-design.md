# Notifiche egui via app overlay + panic screen — Design

**Data:** 2026-06-11
**Stato:** implementato — vedi CHANGELOG/472-26-06-11-notifiche-egui-overlay.md
e il piano docs/superpowers/plans/2026-06-11-notifiche-egui-overlay.md
**Estende:** `2026-06-11-kernel-event-bus-design.md` (v1 implementata, CHANGELOG 471)

## Obiettivo

Le notifiche v1 (toast + modale power) sono disegnate kernel-side col modulo
`decor` (rettangoli + bitmap font): funzionali ma brutte. Questa v2 le rende
**egui vero** spostando il rendering in una app WASM dedicata (`notify`),
compositata come **overlay full-screen trasparente sopra tutte le finestre**.
Il decor v1 resta come **fallback** (notify assente/morta) — la garanzia
"notifiche critiche anche senza desktop egui" della spec v1 è invariata.

Tre livelli di rendering:

| Livello | Chi disegna | Quando |
|---|---|---|
| Toast INFO/WARN + modale power | app `notify` (egui, overlay) | notify viva |
| Fallback | decor kernel v1 (invariato) | notify assente/morta |
| FATAL (kernel panic) | panic handler, direct-framebuffer | sempre, solo testo tecnico |

Principi:

- **Kernel = meccanismo, app = estetica.** Il kernel espone il bus (già
  pronto: ring con cursori per-lettore) e lo stato power; la app decide look,
  animazioni, layout.
- **L'enforcement non cambia**: `power_enforce_task` spegne comunque; il
  modale egui è solo visualizzazione, come il decor.
- **Un panic non passa dal bus** (niente executor/WASM vivi in panic): path
  sincrono separato, direct-framebuffer, solo contenuto tecnico.

## 1. Finestra overlay (kernel, `wm.rs`)

- `Window.overlay: bool` — speculare a `bg`, invertito di z:
  - compositata **per ultima** (sopra ogni finestra normale), forzata
    full-screen `(0,0,sw,sh)` a ogni `present()` (come la bg);
  - **alpha-blend** invece di copy opaca (vedi §1.1);
  - esclusa da: `window_at` (hit-test normale), `window_list` (taskbar),
    raise/focus/drag/minimize/maximize; mai `focused`;
  - niente ombra (`shadow: false`), niente manifest (mai nel launcher);
  - reap normale su crash (`close_requested`): alla morte si torna al
    fallback decor. **Niente respawn automatico in v2** (si ricrea col
    prossimo avvio del compositor).
- Host fn **`wm.set_overlay()`** (speculare a `set_background`): flagga la
  finestra chiamante come overlay via `overlay_request` in `WmState`,
  applicata deferred nel run loop. Un solo overlay: richieste successive da
  altre finestre sono ignorate con `bwarn`.
- Spawn: `Compositor::new` dopo la shell prova `module_by_name("notify")` →
  `spawn_named("notify", m)`. Assente → si resta in fallback decor.

### 1.1 Compositing con alpha

`compose.rs`: `WinDesc` guadagna `blend: bool`. Nel band kernel, per un desc
con `blend = true` la riga non è `copy_nonoverlapping` ma blend per-pixel
**src-over con sorgente premoltiplicata** (tiny-skia produce alpha
premoltiplicato):

```
out.r = src.r + dst.r * (255 - src.a) / 255   (idem g, b; X ignorato)
```

Tutto intero → il composite a bande resta bit-identico al riferimento seriale
(vincolo del test SP4). Le finestre normali e la bg restano copy opaco
(`blend: false`, zero costo aggiunto).

## 2. Input routing (kernel, run loop)

Ordine di valutazione per ogni evento, PRIMA del routing v1:

1. **Modal grab**: se `power::pending().is_some()` E overlay viva → TUTTO
   l'input (mouse + tastiera) va in coda all'overlay. Le finestre normali non
   ricevono nulla (stessa semantica del modale v1).
2. **Hit-test per-pixel** (solo eventi mouse, niente grab attivo): se il
   cursore cade su un pixel della surface committata dell'overlay con
   **alpha ≥ 32**, l'evento va all'overlay (move/press/release/wheel,
   coordinate window-local = screen, è full-screen). Alpha < 32 (trasparente
   o quasi, es. coda d'ombra egui) → routing normale alle finestre sotto.
   La lettura è un accesso a un pixel della surface in `store.data().win`
   (pixels RGBA8888, già accessibile al compositor).
3. Tastiera senza grab: alle finestre normali (i toast non usano tastiera).

I path v1 (`modal_input`, `toast_at`, `drain_kevents`→toast, `tick_modal`)
restano ATTIVI SOLO quando l'overlay non è viva (fallback). Con overlay viva
il kernel continua a drenare il suo cursore (gli serve per accorgersi dei
nuovi eventi) ma invece di creare toast/modale **sveglia l'overlay**
(`last_active_frame = frame_no`), che li leggerà col proprio cursore.

## 3. API host nuove

Registrate in `wm.rs::add_to_linker<T: HasWindow>` (serve lo stato finestra).
Documentazione **nello stesso commit**: `docs/api/sys.md`, `docs/api/wm.md`,
extern in `ruos-desktop/crates/ruos-window/src/lib.rs`.

### `sys.events_poll(buf_ptr) -> i32` (modulo "sys" — è la v2 della spec v1)

Un evento del kevent bus per chiamata, dal cursore della finestra chiamante
(`WmState.kev_cursor`, init a `kevent::current_seq()` alla creazione della
finestra). Ritorna `1` = record scritto, `0` = niente di nuovo. Record fisso
**64 byte** little-endian:

| Offset | Tipo | Campo |
|---|---|---|
| 0 | u64 | seq |
| 8 | u16 | kind |
| 10 | u8 | severity |
| 11 | u8 | pad |
| 12 | u32 | ts_ticks |
| 16 | 4×u32 | payload |
| 32 | 32B | nome UTF-8 NUL-padded (vuoto se assente/sovrascritto) |

Gap rilevato (`read_since` → lost>0): il kernel scrive PRIMA un record
sintetico `SUBSCRIBER_OVERFLOW{lost_lo, lost_hi}` poi gli eventi reali.
API generale: qualunque finestra può usarla (multi-lettore nativo del ring).

### `wm.power_pending() -> i64`

`0` = nessuna richiesta; altrimenti `(kind << 32) | tick_rimanenti` con
`kind`: `1` poweroff, `2` reboot. Fonte di verità del countdown.

### `wm.power_cancel()`

`power::cancel()` — annulla la richiesta pendente (no-op se assente).

### `wm.set_overlay()`

Vedi §1.

## 4. SDK (`ruos-window`) + `gui-core`

- `gui-core::raster::Renderer`: nuovo setter `pub fn set_clear(&mut self,
  rgba_premul: [u8; 4])` (il campo `clear` esiste già, default opaco).
  gui-core resta pura (nessuna dipendenza nuova).
- `ruos-window`:
  - extern nuovi: `sys.events_poll`, `wm.set_overlay`, `wm.power_pending`,
    `wm.power_cancel`;
  - `pub struct KEventRec { seq: u64, kind: u16, severity: u8, ts_ticks: u32,
    payload: [u32; 4], name: String }` + `pub fn events_poll() ->
    Option<KEventRec>` (decodifica il record 64B);
  - `pub fn set_overlay()`, `pub fn power_pending() -> Option<(PowerKind,
    u32 /* tick */)>` con `pub enum PowerKind { Poweroff, Reboot }`,
    `pub fn power_cancel()`;
  - canvas trasparente via `WindowState::new_overlay()` (costruttore che fa
    `set_clear([0,0,0,0])` una volta) + il `frame_once_bare` esistente —
    nessuna `frame_once_overlay` separata (deviazione implementativa: stessa
    pipeline commit-on-damage, una fn in meno).

## 5. App `notify` (`ruos-desktop/apps/notify-app`)

cdylib wasip1 sul pattern about-app, **senza** `declare_manifest!` (non nel
launcher). Per frame:

1. primo frame: `set_overlay()` + `surface_size()` (deferred finché 0×0);
2. drena `events_poll()` → coda toast (mappatura testo per kind = quella v1:
   APP_CRASHED con causa, APP_FUEL_EXHAUSTED, MEM_LOW, TEST, OVERFLOW,
   default tecnico per kind ignoti);
3. `power_pending()` → stato modale;
4. UI egui:
   - **toast**: `egui::Area` ancorate top-right, `Frame` con rounding,
     ombra, fill semi-trasparente del tema scuro, bordo colorato per
     severity (grigio INFO / ambra WARN); fade-out animato sull'età; click
     sul corpo = dismiss; max 3 visibili + coda FIFO; vita ~5 s da quando
     visibile (parametri v1);
   - **modale**: `egui::Window` centrata non collassabile/movibile
     ("Spegnimento"/"Riavvio", "tra N s" da `power_pending`, bottone
     **Annulla** → `power_cancel()`; `Esc` via `ctx.input` → idem);
5. `stay_awake()` SOLO con toast attivi o modale visibile (animazioni);
   altrimenti la finestra dorme e il kernel la sveglia al prossimo evento.

## 6. Fallback decor (v1, invariato)

Attivo quando nessuna finestra overlay è viva. Transizioni:

- notify muore con toast/pending attivi → al frame dopo il reap il kernel
  riprende a creare toast decor dai NUOVI eventi (i toast già mostrati da
  notify sono persi — accettato) e `tick_modal` decor riappare se
  `pending()` è attivo.
- notify torna (nuovo avvio compositor) → kernel smette di disegnare decor.

## 7. Panic screen (FATAL, kernel-only)

Nel `panic_handler` (`main.rs`), DOPO i sink esistenti (klog/serial/console,
invariati): nuova `gfx::panic_screen(info_msg: &str)` che disegna
**direttamente sul framebuffer lineare** — niente lock bloccanti (i parametri
fb sono atomics: `GFX_VIRT`, `GFX_PITCH`, `GFX_BPP`, `GFX_FMT`, `GFX_W/H`),
niente alloc, IRQ già disabilitati:

- sfondo full-screen rosso scuro (es. `#3a0d0d`), testo col font bitmap
  kernel (`console::font`, raster puro);
- header `KERNEL PANIC`, messaggio + location (`PanicInfo`), core id
  (`cpu_id()`), tick (`timer::ticks()`);
- **tail del klog ring** (~ultime 15 righe, da `klog::read` in un buffer
  fisso) — il contesto tecnico;
- footer: `reboot in 30 s` → busy-wait TSC poi `power::reboot()` (default);
  con feature `panic-halt`: `halted` e hlt-loop (comportamento attuale).

Solo contenuto tecnico, niente egui, niente bus. Se il framebuffer non c'è
(`GFX_VIRT` null) la fn è no-op (resta il path seriale). Funziona sia in
GUI mode sia in console mode (scrive sopra qualunque cosa: stiamo morendo).

## 8. Build (lockstep submodule)

- `ruos-desktop`: nuova crate `apps/notify-app` nel workspace.
- Makefile ruos: regola `build/notify.cwasm` (pattern di `build/about.cwasm`:
  cargo build -p notify-app wasm32-wasip1 + wt-precompile), aggiunta alle
  dipendenze di `iso` e copiata in `build/binstage/notify.cwasm` (→
  `/bin/notify.cwasm`).

## 9. Fuori scope

- Respawn automatico di notify morta.
- Trasparenza/alpha per finestre normali (solo overlay).
- Più overlay contemporanee.
- Suoni, centro notifiche, persistenza, azioni sui toast.
- Panic screen con backtrace simbolico.

## 10. Test e verifica

- `make run-test` resta verde (notify non altera il boot headless; il
  compositor non parte in run-test).
- Negativo headless v1 invariato (`kev-test poweroff` da console: enforcement
  kernel, nessun overlay coinvolto).
- Visivo in `make run` + `compositor`:
  - `kev-test` → toast egui arrotondato top-right, fade-out, click dismiss;
  - click sotto/attraverso aree trasparenti dell'overlay → le finestre
    rispondono normalmente (per-pixel hit-test);
  - bottone power desktop → modale egui countdown + Annulla/Esc; scadenza →
    spegnimento;
  - ISO manipolata senza `notify.cwasm` (o `pkill` della notify se
    applicabile) → fallback decor v1.
- Panic screen: nuovo modo debug `kev-test panic` (host fn `ruos.kev_test`,
  `mode = 4` → `panic!("kev-test: requested panic")`; un panic non è un
  kevent, ma il builtin debug è il posto naturale per innescarlo) → a
  schermo il panic screen con messaggio + klog tail; default reboot dopo
  30 s; con `panic-halt` resta. Aggiornare `docs/api/ruos.md`.

## Parametri

| Parametro | Valore |
|---|---|
| Soglia alpha hit-test overlay | 32 (0..=255) |
| Blend overlay | src-over premoltiplicato, intero |
| Record events_poll | 64 B fisso |
| Toast (vita, max visibili, FIFO) | come v1: ~5 s, 3, coda |
| Countdown power | come v1: 10 s |
| Panic screen: righe klog tail | ~15 |
| Panic screen: reboot dopo | 30 s (busy-wait TSC); `panic-halt` → halt |
