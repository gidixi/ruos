# Desktop app sleep/wake lifecycle — design

**Data:** 2026-06-09
**Topic:** sospensione cooperativa delle app finestra del desktop (compositor
kernel-side): idle → dormi (skip `frame()`), interazione/dati → sveglia. Mai
uccidere un terminale locale per inattività.
**Repo:** ruos (kernel, padre) + ruos-desktop (submodule: bindings `ruos-window`,
1 riga in `system-app`).

## Problema

Due difetti legati all'inattività:

1. **PTY watchdog uccide i terminali GUI locali.** `pty_watchdog_task`
   (`kernel/src/executor/mod.rs:848`) reap-a ogni pair `1..NUM_PAIRS` idle da
   >5 min (`IDLE_LIMIT_TICKS=30000`), escludendo solo il pair 0 (boot shell). Il
   terminale GUI usa un pair ≥1 → lasciarlo fermo (es. **scrollato su a leggere**)
   5 min lo killa: `pair 1 idle 304s — shutting down` → `shell exited`. Perdi
   sessione + scrollback. Il watchdog serviva ai pair SSH *leaked*, non ai
   terminali locali.

2. **Tutte le app girano sempre.** Il run loop del compositor (`wm.rs:1544`)
   chiama `frame()` di **ogni** finestra ad ogni giro (`frame_all`, wm.rs:1375).
   Il damage-gating (`committed`) evita solo il *blit* finale, non l'esecuzione
   WASM di `frame()`. App idle/coperte/minimizzate consumano CPU del GUI core.

Obiettivo: un comportamento **globale** del compositor — le app idle dormono
(la loro `frame()` non viene chiamata), si svegliano su interazione o dati in
arrivo; un'app può forzare l'aggiornamento continuo (override). Il terminale
diventa un caso particolare: non si uccide, si addormenta.

## Decisioni (da brainstorming)

- **Approccio A**: `view`-style wake-flag + wake-condition registrate. È l'unico
  che onora "dati in arrivo (PTY/net)" come sveglia mantenendo lo **skip totale**
  di `frame()` quando idle.
- **Semantica dormi = skip totale** (frame congelata) + **override** per-app.
- **Override = `wm.stay_awake()` chiamato ogni frame** (modello egui
  `request_repaint`). Copre anche il caso "sempre attiva" (monitor): l'app lo
  chiama incondizionatamente. **Nessun flag manifest `continuous`** (scartato:
  zero modifiche al parser manifest/launcher).
- **Trigger di sveglia**: focus/click, mouse hover, tastiera, dati in arrivo
  (PTY/net), override. Tutti convergono su un'unica decisione `awake` per finestra.
- **Watchdog**: smette di uccidere i pair `LocalGui`; reap solo i pair `Ssh`. Il
  leak dei pair GUI è chiuso in modo **deterministico** dal lifecycle della
  finestra (reap finestra → shutdown pair legato), non a timeout.

## Architettura

```
run loop (wm.rs:1544)
  ├─ drain input → eventi nelle code finestra (focus/hover/click/key)   [sveglia]
  ├─ azzera stay_awake_request + committed                              [pre-frame]
  ├─ frame_all():  per ogni win → compute_awake(win) ? frame() : skip   [gating]
  ├─ deferred (spawn/bg/close…)
  └─ present(): blit solo se any_committed (damage-gated, invariato)

pty/mod.rs:  PtyOrigin{Free,Ssh,LocalGui} per pair  → watchdog esenta LocalGui
ruos-window: wm.stay_awake(), wm.wake_on_pty(idx)  + auto-bind in RuosTermIo
```

### 1. Stato awake (per finestra)

Campi nuovi in `WmState` (stato guest-facing, `wm.rs:589`):
```rust
/// Override dinamico: il guest ha chiamato wm.stay_awake() in questo frame →
/// resta sveglio il prossimo. Azzerato dal run loop prima di ogni frame_all.
pub stay_awake_request: bool,
/// Risorsa PTY legata via wm.wake_on_pty(idx): -1 = nessuna. Il compositor
/// sveglia la finestra dormiente se quel pair ha output non drenato.
pub wake_pty: i32,
```

Campi nuovi in `Window` (wrapper compositor kernel-side, `wm.rs:916`):
```rust
pub awake: bool,            // calcolato ogni loop (diagnostica/telemetria)
pub last_active_frame: u32, // ultimo frame con attività (grace/debounce)
pub framed_once: bool,      // ha già eseguito _initialize + prima frame()
```

> **Drop dal brainstorming**: niente `continuous` in `WmState` né nella macro
> `declare_manifest!` — l'override è interamente `stay_awake_request`.

### 2. Decisione `awake` (logica pura)

```rust
/// Decide se una finestra deve girare questo frame. Pura → boot-check-abile.
fn should_wake(framed_once: bool, has_events: bool, stay_awake: bool,
               frame_no: u32, last_active: u32, grace: u32,
               pty_has_output: bool) -> bool {
    !framed_once
        || has_events
        || stay_awake
        || pty_has_output
        || frame_no.wrapping_sub(last_active) < grace
}
```

`compute_awake(&Window, frame_no) -> bool` adatta i primitivi:
- `has_events = !win.events.is_empty()`  (focus/hover/click/key già accodati),
- `stay_awake = win.stay_awake_request`,
- `pty_has_output = win.wake_pty >= 0 && pty::master_output_len(win.wake_pty as usize) > 0`,
- `framed_once`, `last_active_frame` dal `Window`.

`GRACE_FRAMES` piccolo (es. 6 ≈ poche decine di ms @ wake-rate) per evitare
flapping sleep↔wake e dare un frame di coda dopo l'ultima attività.

La finestra **focused ma idle** dorme; un tasto entra nella sua coda eventi →
sveglia al giro dopo. Il cursore-blink si ferma mentre dorme; un'app che lo vuole
vivo chiama `wm.stay_awake()` mentre focused.

### 3. Gating in `frame_all` (wm.rs:1375)

```rust
fn frame_all(&mut self) {
    let fno = self.frame_no;
    for w in self.wins.iter_mut() {
        if !compute_awake(w, fno) {
            w.awake = false;
            continue;                 // dormiente: niente frame(), surface cache invariata
        }
        w.awake = true;
        w.framed_once = true;
        w.last_active_frame = fno;
        // ... chiamata frame() esistente (get_typed_func "frame") + bootstrap size ...
    }
}
```
Richiede `frame_no` accessibile in `frame_all` (oggi è `loop`-local nel `run`):
spostarlo in un campo `Compositor.frame_no` (bump per giro). Geometria/focus
cambiati (drag/raise/maximize/minimize/spawn) settano `last_active_frame = fno`
sulla finestra coinvolta → entra in grace → si ridisegna.

L'azzeramento di `stay_awake_request` avviene col reset di `committed`
(wm.rs:1637), **prima** di `frame_all`.

`present()` invariato: le dormienti non committano → niente blit se nessuno
disegna (già damage-gated); le surface dormienti restano composited dalla cache
`pixels`.

### 4. ABI: host fn + bindings + auto-bind PTY

**Kernel** (`wm.rs add_to_linker`, pattern `func_wrap` esistente):
```rust
linker.func_wrap("wm", "stay_awake",
    |mut c: Caller<'_, T>| { c.data_mut().win().stay_awake_request = true; })?;
linker.func_wrap("wm", "wake_on_pty",
    |mut c: Caller<'_, T>, idx: i32| { c.data_mut().win().wake_pty = idx; })?;
```

**ruos-window** (`mod wm extern "C"`, lib.rs:22):
```rust
pub fn stay_awake();
pub fn wake_on_pty(idx: i32);
```
+ wrapper safe:
```rust
/// Tieni sveglia QUESTA finestra il prossimo frame (chiamala ogni frame finché
/// vuoi aggiornamento continuo: clock, animazioni, monitor live).
pub fn stay_awake() { unsafe { wm::stay_awake() } }
```

**Auto-bind PTY** in `RuosTermIo` (lib.rs:75) — il terminale si sveglia su output
shell senza codice app:
```rust
fn term_open(&mut self) -> Option<TermHandle> {
    let h = unsafe { term::open() };
    if h < 0 { None } else { unsafe { wm::wake_on_pty(h); } Some(h) }
}
fn term_close(&mut self, h: TermHandle) {
    unsafe { wm::wake_on_pty(-1); term::close(h); }
}
```

**App ad aggiornamento continuo** (es. `system-app`): chiama
`ruos_window::stay_awake()` in cima alla sua `frame()` (una riga, opt-in esplicito).

### 5. PTY watchdog: esenta i terminali locali

**Tag origine pair** (`pty/mod.rs`):
```rust
#[derive(Clone, Copy, PartialEq)]
pub enum PtyOrigin { Free, Ssh, LocalGui }
// static ORIGIN: [AtomicU8; NUM_PAIRS] (0 Free,1 Ssh,2 LocalGui); release()→Free.
pub fn set_origin(idx: usize, o: PtyOrigin);
pub fn origin(idx: usize) -> PtyOrigin;
```
- `term::open` (GUI, term.rs:22) → dopo `try_claim`: `set_origin(idx, LocalGui)`.
- claim SSH (in `ssh/`) → `set_origin(idx, Ssh)`.

**Watchdog** (`executor/mod.rs:859`):
```rust
for idx in 1..NUM_PAIRS {
    if !is_claimed(idx) || is_shutdown(idx) { continue; }
    if pty::origin(idx) != PtyOrigin::Ssh { continue; } // solo leak SSH remoti
    if now.saturating_sub(last_activity(idx)) > IDLE_LIMIT_TICKS {
        request_shutdown(idx); // log invariato
    }
}
```

**Lifecycle-based release** (chiude il leak dei pair GUI, sostituisce la
safety-net a timeout): nel `reap()` del compositor (wm.rs:1333), quando una
finestra muore, se `win.wake_pty >= 0`:
```rust
crate::pty::request_shutdown(win.wake_pty as usize); // shell esce, pair liberato
```

### Error handling / edge

- `frame()` che ritorna `Err` → `close_requested` (invariato). Una finestra
  dormiente non chiama `frame()` quindi non può fallire mentre dorme.
- `wake_pty` con idx fuori range / non più claimed → `master_output_len` ritorna
  0 (guardia esistente) → la finestra resta dormiente: corretto (niente dati).
- Prima frame / `_initialize`: `framed_once=false` forza il primo giro sveglio.
- Spawn di una nuova finestra: parte `framed_once=false` → sveglia finché disegna
  + grace.
- `wrapping_sub` su `frame_no` gestisce l'overflow del contatore u32.

## Testing

Kernel `no_std`, **niente `cargo test`** → **boot-checks** (`make iso
CARGO_FEATURES=boot-checks`, asserzioni via seriale), sul pattern `selftest_*`
esistente (wm.rs:1895+).

**Boot-check nuovi** (`wm.rs`):
- `selftest_idle_sleeps`: finestra test, nessun evento → dopo `GRACE_FRAMES`
  `frame()` NON gira (il contatore `tick` smette di avanzare).
- `selftest_event_wakes`: push `GfxEvt` in coda → frame successivo `awake` →
  `tick` riprende.
- `selftest_stay_awake`: finestra che chiama `wm.stay_awake()` ogni frame → `tick`
  avanza sempre.
- `selftest_pty_wake`: lega `wake_pty`, inietta output nel master del pair → la
  finestra dormiente si sveglia.

**Watchdog** — helper puro + boot-check:
- `should_reap(origin, idle_exceeded) = origin==Ssh && idle_exceeded`.
- `selftest_watchdog_skips_local`: pair `LocalGui` idle oltre limite → NON
  shutdown; pair `Ssh` idle → shutdown.

**ruos-window / gui-core**: solo FFI (auto-bind, wrapper) → nessun test nuovo;
`cargo test -p gui-core` resta verde.

**Verifica manuale** (QEMU/VBox/hardware reale):
1. Terminale idle 6+ min → **non** ucciso. Tasto → risponde subito.
2. Idle: `sleep 300; echo done` → dopo 5 min "done" appare (PTY sveglia la
   finestra dormiente).
3. System monitor non-focused → continua ad aggiornarsi (`stay_awake`).
4. Desktop idle con più finestre → carico GUI core scende (frame() skippate);
   osserva via system monitor / netconsole.

## Fuori scope (YAGNI)

- Flag manifest `continuous` (sostituito da `stay_awake()` per-frame).
- Sleep di app `stay_awake` quando minimizzate/totalmente occluse (raffinamento
  futuro: anche le continuous potrebbero saltare se invisibili).
- Throttle a rate basso (scelto skip totale).
- Wake su socket/net generico oltre al PTY (stesso meccanismo `wake_on_*`
  estendibile in seguito; per ora solo `wake_on_pty`).
- Coalescing/priorità tra finestre sveglie (tutte le sveglie girano ogni frame).
