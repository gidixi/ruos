# Compositor / Window Manager

> **Stato:** bozza
> **Aggiornato:** 2026-06-08
> **Fonti:** `kernel/src/wasm/wt/wm.rs`, `kernel/src/wasm/wt/compose.rs`,
> `kernel/src/gfx/mod.rs`
> **Spec collegate:** `docs/superpowers/specs/2026-06-05-egui-compositor-sp-e-apps-as-windows-design.md`,
> `docs/superpowers/specs/2026-06-04-egui-desktop-wasmtime-aot-design.md`

## Cos'è

Il **compositor** è il window manager di ruOS: vive **dentro il kernel** ed è la
GUI a regime. Ogni finestra a schermo è un'**app WebAssembly separata** (un
`.cwasm` precompilato, eseguito da Wasmtime AOT), con la propria linear memory.
Il compositor possiede la lista delle finestre, lo z-order, il focus e l'input
routing; compone le surface delle app nel framebuffer e fa il blit.

È l'unico consumer della coda di input del kernel (`gfx::pop`): folda il mouse,
fa hit-test, assegna il focus al click, traduce le coordinate in window-local e
spinge gli eventi **solo** nella coda della finestra giusta. Le app sono cieche
l'una verso l'altra — l'isolamento è garantito dalla sandbox WASM + dalle code
per-finestra.

Modello mentale: **"una finestra = un'app WASM"** ([Model A](#model-a-una-finestra-una-app)).
Niente decorazioni kernel: ogni app disegna la propria title bar, [X] e contenuto
(CSD, [client-side decorations](#csd-decorazioni-lato-client)).

## Dove vive

| File | Ruolo |
|------|-------|
| `kernel/src/wasm/wt/wm.rs` | Tipi `Compositor`/`Window`/`WmState`, loop `run`, host fn `wm`, launcher dinamico |
| `kernel/src/wasm/wt/compose.rs` | Kernel di compositing per banda (`composite_band`, `WinDesc`) |
| `kernel/src/gfx/mod.rs` | Framebuffer, coda eventi, `fold_mouse`, `blit`, cursore software |
| `ruos-desktop/crates/ruos-window` | SDK lato-app: bindings `wm` + `frame_once` + titlebar CSD |
| `ruos-desktop/apps/*` | Le app finestra (shell, about, files, terminal, system, compositor-app) |

L'entry point è `run_compositor_gate` (`wm.rs`), che il router dell'executor
chiama per nome — **non rinominare**.

## Model A: una finestra, una app

Non c'è un monolite GUI. La gerarchia a runtime:

```
shell.cwasm  (finestra di background: wallpaper + panel + launcher)
   │  wm.spawn("about")
   ├──► about.cwasm     ┐
   ├──► files.cwasm     │ ogni app = cdylib thin su ruos-window SDK
   ├──► terminal.cwasm  │   └─ gui-core (egui + raster tiny-skia) → RGBA8888
   ├──► system.cwasm    │
   └──► egui-demo.cwasm ┘
```

La **shell** è la prima finestra: disegna sfondo + barra/launcher e si auto-flagga
come background (`wm.set_background`) al primo frame; mappa i click del launcher su
`wm.spawn` / `wm.poweroff`. Ogni **app finestra** è un cdylib `wasm32-wasip1` con
uno `static mut` di stato e un export `frame()` che il compositor chiama ogni giro.

## Modello e tipi

Definiti in `wm.rs`:

- **`Compositor`** — possiede tutto:
  - `wins: Vec<Window>` — **l'ordine del Vec È lo z-order**: indice 0 = fondo,
    ultimo = cima. Non esiste un campo `z`; `raise(idx)` sposta la finestra in coda.
  - `focused: usize` — indice della finestra con focus.
  - `drag: Option<DragState>` — move interattivo in corso (grab offset).
  - `backbuf: Vec<u8>` — back-buffer RGBX a dimensione schermo.
  - `free_ids` / `next_id` — riciclo dei window-id.
  - `dirty: bool` — damage di geometria/window-set (vedi [present-gating](#present-gating-lavoro-zero-da-idle)).
- **`Window`** — una finestra = un'istanza reattore persistente:
  - `id`, `store: Store<AppState>` (la linear memory del guest + il suo `WmState`),
    `inst`, `rect (x,y,w,h)`, `title`, `pid`.
  - flag: `focused`, `bg`, `minimized`, `maximized` (+ `saved_rect`), `sized`,
    `alive`.
- **`WmState`** (dentro `AppState`, lo store data della finestra) — il canale di
  comunicazione app↔compositor. I pixel committati (`pixels`, `win_w`, `win_h`),
  la coda eventi privata (`events`), e i **flag di richiesta** che il guest setta
  via host fn e che il loop processa *dopo* il frame: `close_requested`,
  `spawn_request`, `bg_request`, `move_requested`, `minimize_request`,
  `maximize_request`, `activate_request`, `target_w/h`, `committed`.

`AppState` incapsula **sia** la capability WASI (`WtState` — perché un guest egui
std/wasip1 ha bisogno del runtime std) **sia** lo stato finestra (`WmState`).

## Comportamento a runtime

Il cuore è `Compositor::run` (`wm.rs`): possiede la CPU, non ritorna mai. Ogni
giro del loop:

1. **heartbeat** — bump del battito del GUI core (il supervisor "6-detect" non lo
   silenzia mentre possiede la CPU).
2. **`reap()`** — rimuove ogni finestra che ha chiesto la chiusura (`close_requested`
   dal guest, o il path [X]). `remove_at` droppa Store+Instance → libera la linear
   memory del guest e il buffer surface, deregistra il proc, ricicla il window-id.
3. **`refresh_app_catalog()`** — scan del [launcher dinamico](#launcher-dinamico-app-auto-descrittive),
   throttlato a ~1 Hz.
4. **input** — `fold_mouse()` poi drena `gfx::pop()` (unico consumer):
   - **mousemove**: se c'è un drag attivo, muove la finestra trascinata
     (`drag_to`) e NON inoltra agli app (egui combatterebbe il move); altrimenti
     inoltra un move window-local alla finestra topmost sotto il cursore (hover),
     o alla finestra `bg` se siamo sul desktop nudo.
   - **bottone sinistro** (edge-tracked): press → `on_left_down` (raise + focus
     della topmost non-`bg` sotto il cursore, inoltro del press posizionato);
     miss di ogni finestra → fallthrough alla `bg`. Release → sempre inoltrato
     (così il pointer egui non resta "premuto"), instradato alla finestra
     trascinata / focused / bg secondo il contesto.
   - **tasto**: in coda alla finestra `focused`.
5. **clear damage** — azzera `committed` su tutte le finestre (lo re-setta `wm.commit`).
6. **`frame_all()`** — chiama l'export `frame()` di ogni finestra. L'app drena la
   sua coda via `wm.poll_event`, ridisegna, e fa `wm.commit`. Un `frame()` che
   ritorna `Err` (trap / `panic=abort` / `proc_exit`) → `close_requested`: rete di
   sicurezza, la finestra rotta viene reaped invece di restare congelata. Il primo
   commit stabilisce la dimensione finestra ("configure bootstrap", flag `sized`).
7. **richieste deferred** — processate QUI, dopo `frame_all`, **mai** mid-iterazione
   di `wins`: bg-pin, spawn (drena i nomi, `module_by_name`, `spawn_named`),
   start-move → `DragState`, minimize/maximize/restore, activate (taskbar).
8. **snapshot taskbar** — pubblica `WINDOW_SNAPSHOT` (id, flags, title delle
   finestre non-bg) per la host fn `wm.window_list`.
9. **`present()`** — [compositing](#compositing-parallelo-a-bande-smp), ma solo se
   [serve](#present-gating-lavoro-zero-da-idle).
10. **`hlt`** — parcheggia il core fino alla prossima IRQ (timer 100 Hz / input),
    invece di busy-spin.

### Input routing e focus

- **Click-to-focus**: `window_at` cerca la topmost non-`bg`, non-`minimized` sotto
  il punto. Il press la fa `raise` + `set_focus` e le inoltra l'evento.
- Gli eventi vanno **solo** alla coda della finestra giusta; un'app vede solo i
  propri eventi (è identificata dal proprio `Store`).
- Le coordinate sono tradotte screen → window-local prima dell'inoltro.
- La **finestra `bg`** è il fallthrough dell'input: un click che manca ogni
  finestra normale va lì (così il panel/launcher della shell riceve i click) senza
  raise né focus.
- Il focus si vede dal colore della title bar (la disegna l'app); niente bordo di
  focus kernel.

### Compositing parallelo a bande (SMP)

`present()` (`wm.rs`) → `dispatch_bands` (`wm.rs`) → `composite_band` (`compose.rs`):

1. Costruisce i descrittori di footprint `WinDesc` in ordine bottom→top
   (l'ordine di `wins`), con la finestra `bg` **prima** (z-fondo, forzata a
   `(0,0,sw,sh)`, senza ombra; le finestre normali proiettano ombra).
   **Leva 0**: ogni descrittore *prende in prestito* la surface committata in
   place (nessun clone) — i pixel vivono nello Store e non vengono toccati fino al
   join, quindi i raw pointer restano validi per tutto il compositing parallelo.
2. Divide lo schermo in **bande orizzontali disgiunte**, una per core online
   (cap `MAX_BANDS = 16`). Una pool-job per banda → eseguita sugli AP via
   `smp::pool`. Bande disgiunte ⇒ nessun job tocca lo stesso byte del back-buffer.
3. Fallback: con ≤1 CPU (o pool piena) le bande si compongono inline sul BSP.
4. **Join** di tutti i job, poi **una sola blit** seriale del back-buffer nel
   framebuffer (`gfx::blit`); il cursore software è ricomposto a parte.

Ogni banda azzera al `DESKTOP_BG` prima di comporre, così drag/close non lasciano
ghost. `COMPOSITE_CORE_MASK` registra quali core hanno composto (marker di
boot-check per provare il multi-core).

### Present-gating: lavoro zero da idle

`present()` gira **solo se qualcosa è cambiato**: una finestra ha committato una
nuova surface (`any_committed`) **oppure** geometria/window-set è cambiato
(`self.dirty` — settato da raise, drag, spawn, reap, bg-pin, maximize…). Un
desktop fermo salta tutto il composite+blit: il framebuffer tiene l'ultimo frame
e il cursore è mantenuto a parte da `gfx`. Con l'`hlt` finale, un desktop idle fa
~zero lavoro per tick.

Eccezione **warm-up**: i primi `WARMUP_FRAMES` (90) sono forzati a presentare, così
la pool SMP si scalda (gli AP impiegano qualche frame a raccogliere i job) e il
marker "composite cores" (al frame 30) osserva attività multi-core reale.

### Core dedicato (hand-off SMP)

Il compositor può girare su un **core dedicato**: `gui_worker_loop` (chiamato da
`ap_entry` quando il ruolo del core è `GuiCompositor`) attende su una mailbox
atomica (`COMPOSITOR_MAILBOX`) che il BSP pubblichi i byte del `compositor.cwasm`,
poi esegue `run_compositor_gate` per sempre su quel core. Il BSP resta libero di
pollare net/usb/ssh nel suo executor.

## Contratti

### CSD: decorazioni lato client

Il kernel **non** disegna decorazioni. Ogni app disegna la propria title bar, i
pulsanti ([X], i "traffic lights" minimize/maximize) e il contenuto, tutto via
egui. Il kernel fa solo window management: raise/focus/drag/close su richiesta del
guest. L'unico rasterizer kernel rimasto è il modulo `decor` (rect pieni + testo
bitmap) usato dalla striscia del launcher.

### Host fn `wm` (ABI app↔compositor)

Registrate da `add_to_linker` (`wm.rs`), import `extern "C"` raw (non WIT su
questo path). Le principali:

| Host fn | Significato |
|---------|-------------|
| `commit(ptr,len,w,h)` | l'app consegna la sua surface RGBA8888 → `WmState.pixels` (+damage) |
| `poll_event(retptr)` | drena UN evento dalla coda della finestra (record da 20 byte) |
| `app_id() -> u32` | il window-id di questa istanza |
| `close()` | chiedi la chiusura di questa finestra |
| `start_move()` | grab della title bar → move interattivo kernel-driven |
| `spawn(ptr,len)` | lancia `/bin/<name>.cwasm` come nuova finestra (deferred) |
| `set_background()` | flagga questa finestra come background full-screen |
| `minimize()` / `toggle_maximize()` | i dot giallo/verde della CSD |
| `activate(id)` | (taskbar) un-minimize + raise + focus della finestra `id` |
| `window_list(ptr,max)` | snapshot taskbar delle finestre non-bg |
| `app_list(ptr,max)` | catalogo launcher (app auto-descrittive) |
| `window_size() -> i64` | dimensione assegnata dal kernel (configure) |
| `surface_size() -> i64` | dimensione full framebuffer |
| `wall_seconds() -> f64` | secondi monotoni da boot (per `RawInput.time` egui) |
| `poweroff()` | spegni la macchina |

Le richieste che mutano `wins` (spawn, close, bg, move, minimize, maximize,
activate) sono **deferred**: l'host fn setta un flag/accoda; il loop le applica
dopo `frame_all`, mai mentre itera `wins`.

### Launcher dinamico (app auto-descrittive)

Un'app appare nel launcher **se e solo se** esporta `manifest() -> i64` (packed
`ptr<<32 | len` di un record UTF-8 `id␟title`). `scan_apps` (`wm.rs`) scandisce
`/bin` e `/mnt/apps` per `*.cwasm`, instanzia ciascuna su uno store usa-e-getta,
legge il manifest e lo memoizza (`MANIFEST_CACHE`). Droppi un `.cwasm` con
manifest in `/mnt/apps` → appare nel launcher entro ~1 s, **senza rebuild**. La
shell legge il catalogo via `wm.app_list`. Le app senza manifest (shell,
compositor, comandi WASI puri) sono semplicemente assenti dal launcher — l'export
È l'opt-in.

## Vincoli e limiti

- **`MAX_WINDOWS = 8`** finestre vive simultaneamente. Ogni istanza egui pesa
  ~48 MB di heap: shell + 4 app = 5 istanze ≈ 240 MB, sotto i 256 MB di heap ma di
  poco. Oltre il budget, `spawn_named` rifiuta in modo graceful.
- **Single-thread per finestra**: il kernel chiama `frame()` serialmente per
  finestra, quindi lo `static mut` di stato dell'app è safe senza lock.
- Le finestre nascono a una dimensione placeholder (320×240) in posizione a
  cascata; la dimensione reale è adottata al primo commit (`sized`), poi la possiede
  il kernel (maximize/restore aggiornano `rect` + `target`).
- **`cld` prima di ogni `rep movs`**: la SysV ABI richiede DF=0; il codice
  cranelift/Rust usa `rep movs`, che gira all'indietro con DF=1 corrompendo i dati.
  Ogni sito che instanzia/chiama un guest fa `asm!("cld")`.

## Insidie / note

- L'ordine di `wins` È lo z-order — non cercare un campo `z`. `raise` rimuove e
  ri-pusha, e aggiusta `focused` di conseguenza.
- `remove_at` è l'**unico** punto da cui una finestra lascia `wins` (close e reap
  ci passano entrambi): garantisce proc-unregister + riciclo id esattamente una volta.
- La finestra `bg` non è mai topmost in `window_at`, quindi vede solo il path di
  fallthrough; non è mai raised/focused/mossa/chiusa.
- Sotto CSD la hit-rect deve seguire la dimensione *committata* (`win_w×win_h`),
  non il placeholder, altrimenti un click sulla [X] dell'app (oltre il bordo
  placeholder) non verrebbe mai instradato.

## Vedi anche

- [Architettura — panoramica](../architecture/overview.md)
- [Indice della wiki](../README.md)
- Spec: `docs/superpowers/specs/2026-06-05-egui-compositor-sp-e-apps-as-windows-design.md`
