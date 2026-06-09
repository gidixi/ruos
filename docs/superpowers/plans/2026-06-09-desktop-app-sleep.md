# Desktop App Sleep/Wake Lifecycle — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Le app finestra del desktop dormono quando idle (la loro `frame()` non viene chiamata dal compositor) e si svegliano su interazione (focus/hover/click/tastiera) o dati in arrivo (PTY); un'app può forzare l'aggiornamento continuo con `wm.stay_awake()`. Il PTY watchdog smette di uccidere i terminali locali.

**Architecture:** Approccio A (spec `docs/superpowers/specs/2026-06-09-desktop-app-sleep-design.md`). Il compositor calcola un flag `awake` per finestra ogni giro e salta `frame()` per le dormienti; le sorgenti di sveglia (eventi già accodati, override `stay_awake`, output PTY legato via `wake_on_pty`) confluiscono in una decisione pura `should_wake`. Il PTY watchdog reap-a solo i pair `Ssh`; i pair `LocalGui` sono liberati dal lifecycle della finestra.

**Tech Stack:** Rust `no_std` (kernel ruos, target `x86_64-unknown-none`, build-std), Wasmtime AOT, egui/wasm32-wasip1 (app submodule). **Nessun `cargo test` sul kernel** → verifica per task = compile via WSL; la logica è coperta da **boot-checks** (Task 9) + verifica manuale (Task 10).

**Build/verify commands (eseguiti via WSL — la toolchain kernel vive lì):**

- Kernel compile (feedback veloce per task kernel):
  ```bash
  wsl -d Ubuntu-22.04 -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/w/Work/GitHub/ruos/kernel && cargo build --release 2>&1 | tail -8'
  ```
- Kernel compile con boot-checks (Task 9):
  ```bash
  wsl -d Ubuntu-22.04 -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/w/Work/GitHub/ruos/kernel && cargo build --release --features boot-checks 2>&1 | tail -8'
  ```
- App submodule compile (Task 7/8):
  ```bash
  wsl -d Ubuntu-22.04 -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/w/Work/GitHub/ruos/ruos-desktop && cargo build -p terminal-app -p system-app --target wasm32-wasip1 --release 2>&1 | tail -8'
  ```
- ISO + boot-check headless test (Task 9/10):
  ```bash
  wsl -d Ubuntu-22.04 -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/w/Work/GitHub/ruos && make iso CARGO_FEATURES=boot-checks && make run-test'
  ```

**Repo / branch:** lavoro su `main` in **entrambi** i repo (ruos padre + ruos-desktop submodule), nessun branch nuovo (autorizzato dall'utente). Regola padre: ogni commit nel padre richiede una entry `CHANGELOG/NN-...`. Numerazione: il massimo attuale è `367` → parti da `368`.

---

## File Structure

- `kernel/src/pty/mod.rs` — **Modify.** `PtyOrigin` enum + array `ORIGIN` + `set_origin`/`origin`; `release` resetta a `Free`.
- `kernel/src/wasm/wt/term.rs` — **Modify.** `term::open` tagga il pair `LocalGui`.
- `kernel/src/ssh/sunset_io.rs` — **Modify.** Il claim SSH tagga il pair `Ssh`.
- `kernel/src/executor/mod.rs` — **Modify.** `should_reap` puro + il watchdog esenta i pair non-`Ssh`.
- `kernel/src/wasm/wt/wm.rs` — **Modify.** Campi `WmState`/`Window`/`Compositor.frame_no`; `should_wake`/`compute_awake`; gating in `frame_all`; host fn `stay_awake`/`wake_on_pty`; release PTY in `reap`; boot-check selftests.
- `kernel/src/wasm/wt/mod.rs` — **Modify.** Wrapper `run_*_demo()` (boot-checks) per i nuovi selftest.
- `ruos-desktop/crates/ruos-window/src/lib.rs` — **Modify.** Extern `stay_awake`/`wake_on_pty`, wrapper safe, auto-bind in `RuosTermIo`.
- `ruos-desktop/apps/system-app/src/lib.rs` — **Modify.** Una riga `ruos_window::stay_awake()` in `frame()`.
- `CHANGELOG/368..` — **Create.** Una entry per ogni commit nel padre.

---

## Task 1: PTY origin tagging

**Files:**
- Modify: `kernel/src/pty/mod.rs` (accanto a `CLAIMED`/`SHUTDOWN`/`LAST_ACTIVITY` ~righe 251-271, e `release` ~righe 231-240)
- Modify: `kernel/src/wasm/wt/term.rs` (`term::open` ~righe 22-30)
- Modify: `kernel/src/ssh/sunset_io.rs` (claim ~riga 411)

- [ ] **Step 1: Aggiungi l'enum + lo stato origine in `pty/mod.rs`**

Dopo la definizione di `LAST_ACTIVITY` (~riga 271) aggiungi:

```rust
/// Origine di un pair claimed: distingue i terminali GUI locali (mai reap-ati
/// per idle: dormono nel compositor) dalle sessioni SSH remote (reap-ate dal
/// watchdog se la sessione leak-a). `Free` = non claimed.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PtyOrigin { Free, Ssh, LocalGui }

/// Per-pair origin. 0=Free, 1=Ssh, 2=LocalGui (vedi `set_origin`/`origin`).
static ORIGIN: [AtomicU8; NUM_PAIRS] = [
    AtomicU8::new(0), AtomicU8::new(0),
    AtomicU8::new(0), AtomicU8::new(0),
];

pub fn set_origin(idx: usize, o: PtyOrigin) {
    if idx >= NUM_PAIRS { return; }
    let v = match o { PtyOrigin::Free => 0, PtyOrigin::Ssh => 1, PtyOrigin::LocalGui => 2 };
    ORIGIN[idx].store(v, Ordering::Relaxed);
}

pub fn origin(idx: usize) -> PtyOrigin {
    if idx >= NUM_PAIRS { return PtyOrigin::Free; }
    match ORIGIN[idx].load(Ordering::Relaxed) {
        1 => PtyOrigin::Ssh,
        2 => PtyOrigin::LocalGui,
        _ => PtyOrigin::Free,
    }
}
```

`AtomicU8` è già importabile dal `use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};` esistente (riga 251): aggiungi `AtomicU8` a quella lista → `use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};`.

- [ ] **Step 2: Resetta l'origine in `release`**

In `release` (`pty/mod.rs` ~riga 231), dopo `SHUTDOWN[idx].store(false, Ordering::SeqCst);` aggiungi:

```rust
    ORIGIN[idx].store(0, Ordering::Relaxed); // back to Free
```

- [ ] **Step 3: Tagga `LocalGui` all'apertura del terminale GUI**

In `kernel/src/wasm/wt/term.rs`, dentro `term::open` (~riga 24), dopo `if crate::pty::try_claim(idx) {` e prima di `spawn_shell_on_pty`:

```rust
            if crate::pty::try_claim(idx) {
                crate::pty::set_origin(idx, crate::pty::PtyOrigin::LocalGui);
                crate::wasm::ssh_spawn::spawn_shell_on_pty(idx);
                return idx as i32;
            }
```

- [ ] **Step 4: Tagga `Ssh` al claim SSH**

In `kernel/src/ssh/sunset_io.rs` (~riga 411), dopo `if crate::pty::try_claim(idx) {`:

```rust
        if crate::pty::try_claim(idx) {
            crate::pty::set_origin(idx, crate::pty::PtyOrigin::Ssh);
            // ... codice esistente (spawn_shell_on_pty ecc.) invariato ...
```

(Mantieni il corpo esistente; aggiungi SOLO la riga `set_origin`.)

- [ ] **Step 5: Compila il kernel**

Run:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/w/Work/GitHub/ruos/kernel && cargo build --release 2>&1 | tail -8'
```
Expected: `Finished \`release\` profile`. Nessun errore.

- [ ] **Step 6: Commit (+ changelog)**

Crea `CHANGELOG/368-26-06-09-pty-origin-tag.md`:
```markdown
# 368 — PTY origin tag (Ssh vs LocalGui)

**Data:** 2026-06-09

## Cosa
`PtyOrigin{Free,Ssh,LocalGui}` per pair; `set_origin`/`origin`; tag a term::open
(LocalGui) e al claim SSH (Ssh); reset in release.

## Perché
Distinguere i terminali GUI locali (da non uccidere per idle) dalle sessioni SSH.

## File toccati
- kernel/src/pty/mod.rs
- kernel/src/wasm/wt/term.rs
- kernel/src/ssh/sunset_io.rs
```
```bash
cd /mnt/w/Work/GitHub/ruos
git add kernel/src/pty/mod.rs kernel/src/wasm/wt/term.rs kernel/src/ssh/sunset_io.rs CHANGELOG/368-26-06-09-pty-origin-tag.md
git commit -m "feat(pty): origin tag Ssh vs LocalGui for the idle watchdog"
```

---

## Task 2: Watchdog esenta i terminali locali

**Files:**
- Modify: `kernel/src/executor/mod.rs` (`pty_watchdog_task` ~righe 848-872)

- [ ] **Step 1: Aggiungi l'helper puro `should_reap`**

Subito SOPRA `async fn pty_watchdog_task()` (~riga 848) aggiungi:

```rust
/// Decide se il watchdog deve reap-are un pair: SOLO le sessioni SSH leak-ate.
/// I terminali GUI locali (`LocalGui`) non vanno mai uccisi per idle — dormono
/// nel compositor e restano vivi finché la finestra esiste. Pura → ovvia.
fn should_reap(origin: crate::pty::PtyOrigin, idle_exceeded: bool) -> bool {
    origin == crate::pty::PtyOrigin::Ssh && idle_exceeded
}
```

- [ ] **Step 2: Usa l'helper nel watchdog**

Nel loop di `pty_watchdog_task` (~righe 859-869), sostituisci il blocco:

```rust
        for idx in 1..crate::pty::NUM_PAIRS {
            if !crate::pty::is_claimed(idx) { continue; }
            if crate::pty::is_shutdown(idx) { continue; }
            let last = crate::pty::last_activity(idx);
            if now.saturating_sub(last) > IDLE_LIMIT_TICKS {
                crate::bwarn!(
                    "pty", "watchdog: pair {} idle {}s — shutting down",
                    idx, now.saturating_sub(last) / 100,
                );
                crate::pty::request_shutdown(idx);
            }
        }
```

con:

```rust
        for idx in 1..crate::pty::NUM_PAIRS {
            if !crate::pty::is_claimed(idx) { continue; }
            if crate::pty::is_shutdown(idx) { continue; }
            let last = crate::pty::last_activity(idx);
            let idle_exceeded = now.saturating_sub(last) > IDLE_LIMIT_TICKS;
            if should_reap(crate::pty::origin(idx), idle_exceeded) {
                crate::bwarn!(
                    "pty", "watchdog: pair {} idle {}s — shutting down",
                    idx, now.saturating_sub(last) / 100,
                );
                crate::pty::request_shutdown(idx);
            }
        }
```

- [ ] **Step 3: Compila il kernel**

Run:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/w/Work/GitHub/ruos/kernel && cargo build --release 2>&1 | tail -8'
```
Expected: `Finished`. Nessun errore.

- [ ] **Step 4: Commit (+ changelog)**

Crea `CHANGELOG/369-26-06-09-watchdog-skip-local-pty.md` (sezioni Cosa/Perché/File come sopra: watchdog reap solo `Ssh`; file `kernel/src/executor/mod.rs`).
```bash
cd /mnt/w/Work/GitHub/ruos
git add kernel/src/executor/mod.rs CHANGELOG/369-26-06-09-watchdog-skip-local-pty.md
git commit -m "feat(pty): watchdog reaps only SSH pairs — never local GUI terminals"
```

---

## Task 3: Campi stato awake (WmState + Window + Compositor.frame_no)

**Files:**
- Modify: `kernel/src/wasm/wt/wm.rs` (`WmState` ~righe 589-631; `Window` ~righe 916-941; `Compositor` struct ~riga 945; i costruttori che inizializzano queste struct)

- [ ] **Step 1: Aggiungi i campi a `WmState`**

In `WmState` (`wm.rs` ~riga 631, dopo `pub target_h: u32,`):

```rust
    /// Override dinamico: il guest ha chiamato `wm.stay_awake()` in questo frame →
    /// resta sveglio il prossimo. Azzerato dal run loop prima di ogni `frame_all`.
    pub stay_awake_request: bool,
    /// Risorsa PTY legata via `wm.wake_on_pty(idx)`: -1 = nessuna. Il compositor
    /// sveglia la finestra dormiente se quel pair ha output non drenato.
    pub wake_pty: i32,
```

- [ ] **Step 2: Inizializza i campi di `WmState` in ogni costruttore**

`WmState` è costruito in almeno due punti (cerca `WmState {` in `wm.rs` — c'è l'init del `Compositor` ~riga 199 e la creazione di ogni finestra spawnata). In OGNI letterale `WmState { ... }` aggiungi:

```rust
            stay_awake_request: false,
            wake_pty: -1,
```

Per trovarli tutti:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && grep -n "WmState {" kernel/src/wasm/wt/wm.rs'
```
Aggiorna ogni occorrenza. (Se ne sfugge una, il compile di Step 5 fallisce con "missing field" → completala.)

- [ ] **Step 3: Aggiungi i campi a `Window`**

In `Window` (`wm.rs` ~riga 940, dopo `pub sized: bool,`):

```rust
    /// SP-sleep: questa finestra ha girato `frame()` in questo loop (diagnostica).
    pub awake: bool,
    /// Ultimo `frame_no` con attività (input/override/dati) — grace/debounce.
    pub last_active_frame: u32,
    /// Ha già eseguito `_initialize` + almeno una `frame()`. Falso → primo giro
    /// sempre sveglio.
    pub framed_once: bool,
```

- [ ] **Step 4: Inizializza i campi di `Window` in ogni costruttore**

Cerca i letterali `Window {` in `wm.rs`:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && grep -n "Window {" kernel/src/wasm/wt/wm.rs'
```
In OGNI letterale `Window { ... }` aggiungi:
```rust
            awake: true,
            last_active_frame: 0,
            framed_once: false,
```
(Una finestra appena creata parte `awake:true, framed_once:false` → gira finché ha disegnato.)

- [ ] **Step 5: Aggiungi `frame_no` a `Compositor`**

In `struct Compositor` (`wm.rs` ~riga 945) aggiungi un campo:
```rust
    /// Contatore di frame del run loop (per il grace/debounce dello sleep).
    pub frame_no: u32,
```
e inizializzalo a `0` nel costruttore del `Compositor` (cerca `Compositor {` per il letterale di init).

- [ ] **Step 6: Compila il kernel**

Run:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/w/Work/GitHub/ruos/kernel && cargo build --release 2>&1 | tail -8'
```
Expected: `Finished`. Se "missing field" → completa il costruttore mancante (Step 2/4/5).

- [ ] **Step 7: Commit (+ changelog)**

Crea `CHANGELOG/370-26-06-09-wm-sleep-state-fields.md` (campi awake/wake_pty/frame_no; file `kernel/src/wasm/wt/wm.rs`).
```bash
cd /mnt/w/Work/GitHub/ruos
git add kernel/src/wasm/wt/wm.rs CHANGELOG/370-26-06-09-wm-sleep-state-fields.md
git commit -m "feat(wm): add per-window sleep state fields + Compositor.frame_no"
```

---

## Task 4: `should_wake` + `compute_awake` + gating in `frame_all`

**Files:**
- Modify: `kernel/src/wasm/wt/wm.rs` (`frame_all` ~righe 1375-1405; il run loop `run` ~righe 1554-1639 per il reset di `stay_awake_request` e il bump di `frame_no`)

- [ ] **Step 1: Aggiungi gli helper `should_wake` + `compute_awake`**

Subito SOPRA `fn frame_all(&mut self)` (~riga 1375), dentro l'`impl Compositor`, aggiungi:

```rust
    /// Numero di frame di grazia dopo l'ultima attività prima di dormire (evita
    /// flapping sleep↔wake; ~poche decine di ms al wake-rate del compositor).
    const GRACE_FRAMES: u32 = 6;
```

E come funzioni libere a livello di modulo (in fondo al file, accanto agli altri helper liberi), aggiungi la decisione PURA:

```rust
/// Decide se una finestra deve girare `frame()` questo giro. Pura → boot-checkabile.
/// Sveglia se: non ha ancora girato (init), ha input in coda, ha l'override
/// `stay_awake`, ha output PTY legato in attesa, o è entro il grace dall'ultima
/// attività.
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

E un metodo che adatta i primitivi (dentro `impl Compositor`, sopra `frame_all`):

```rust
    /// `should_wake` calato su una `Window` concreta + il frame corrente.
    fn compute_awake(w: &Window, frame_no: u32) -> bool {
        let s = w.store.data();
        let has_events = !s.win.events.is_empty();
        let stay_awake = s.win.stay_awake_request;
        let pty_has_output = s.win.wake_pty >= 0
            && crate::pty::master_output_len(s.win.wake_pty as usize) > 0;
        should_wake(w.framed_once, has_events, stay_awake,
                    frame_no, w.last_active_frame, Self::GRACE_FRAMES, pty_has_output)
    }
```

- [ ] **Step 2: Gate in `frame_all`**

Sostituisci il corpo di `fn frame_all(&mut self)` (~riga 1375). Il loop attuale:

```rust
    fn frame_all(&mut self) {
        for w in self.wins.iter_mut() {
            if let Ok(frame) = w.inst.get_typed_func::<(), ()>(&mut w.store, "frame") {
                match frame.call(&mut w.store, ()) {
                    Ok(()) => {}
                    Err(_) => { w.store.data_mut().win.close_requested = true; }
                }
            }
            // ... bootstrap size (cw/ch) invariato ...
```

diventa:

```rust
    fn frame_all(&mut self) {
        let fno = self.frame_no;
        for w in self.wins.iter_mut() {
            if !Self::compute_awake(w, fno) {
                w.awake = false;
                continue; // dormiente: niente frame(); la surface in cache resta valida
            }
            w.awake = true;
            w.framed_once = true;
            w.last_active_frame = fno;
            if let Ok(frame) = w.inst.get_typed_func::<(), ()>(&mut w.store, "frame") {
                match frame.call(&mut w.store, ()) {
                    Ok(()) => {}
                    Err(_) => { w.store.data_mut().win.close_requested = true; }
                }
            }
            // ... bootstrap size (cw/ch) invariato — LASCIA il resto del corpo del loop ...
```

(Mantieni inalterato il blocco bootstrap-size `let (cw, ch) = ...; if cw != 0 && ch != 0 && !w.sized { ... }` che segue, ora dentro il ramo sveglio.)

- [ ] **Step 3: Bump `frame_no` + reset `stay_awake_request` nel run loop**

In `run` (`wm.rs` ~riga 1554), all'inizio del `loop {` (subito dopo `loop {`), aggiungi il bump:

```rust
        loop {
            self.frame_no = self.frame_no.wrapping_add(1);
```

E dove il run loop azzera i flag damage prima di `frame_all` (~riga 1637):

```rust
            for w in self.wins.iter_mut() { w.store.data_mut().win.committed = false; }
```

estendi per azzerare anche l'override dinamico:

```rust
            for w in self.wins.iter_mut() {
                let s = w.store.data_mut();
                s.win.committed = false;
                s.win.stay_awake_request = false;
            }
```

- [ ] **Step 4: Segna attività su cambi di geometria/focus**

Per garantire un redraw dopo drag/raise/focus/maximize/minimize, marca la finestra coinvolta. Nel run loop, ovunque cambi `self.focused` o la `rect`/stato di una finestra a seguito di input (es. in `on_left_down`, `drag_to`, `raise`, maximize/minimize handler), imposta sull'indice coinvolto:

```rust
            self.wins[i].last_active_frame = self.frame_no;
```

Concretamente, aggiungi questa marcatura in `forward_left_button` / `on_left_down` / `drag_to` / `forward_mouse_move` subito dopo aver individuato l'indice `i` della finestra bersaglio (così l'interazione la tiene in grace anche se la coda eventi è già stata drenata). Se un metodo non ha `self.frame_no` in scope perché è `&mut self`, è comunque accessibile (`self.frame_no`). NON marcare la `bg` window in modo speciale: va bene marcarla come le altre.

> Nota: gli eventi input vengono comunque accodati in `win.events` (che `compute_awake` legge), quindi questa marcatura è una cintura-di-sicurezza per il frame della transizione + il grace; se un metodo è troppo intricato per inserirla pulitamente, è accettabile ometterla lì purché l'evento finisca in coda (la sveglia avviene comunque). Documenta dove l'hai messa nel messaggio di commit.

- [ ] **Step 5: Compila il kernel**

Run:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/w/Work/GitHub/ruos/kernel && cargo build --release 2>&1 | tail -8'
```
Expected: `Finished`. Nessun errore/borrow issue (il loop usa `let fno = self.frame_no;` per non prendere in prestito `self` mentre itera `self.wins`).

- [ ] **Step 6: Commit (+ changelog)**

Crea `CHANGELOG/371-26-06-09-wm-frame-sleep-gating.md`.
```bash
cd /mnt/w/Work/GitHub/ruos
git add kernel/src/wasm/wt/wm.rs CHANGELOG/371-26-06-09-wm-frame-sleep-gating.md
git commit -m "feat(wm): skip frame() for idle windows (sleep) — should_wake gating"
```

---

## Task 5: Host fn `wm.stay_awake` + `wm.wake_on_pty`

**Files:**
- Modify: `kernel/src/wasm/wt/wm.rs` (`add_to_linker` ~righe 663-686, accanto agli altri `func_wrap("wm", ...)`)

- [ ] **Step 1: Registra le due host fn**

In `add_to_linker` (`wm.rs`), accanto a `wm.close` (~riga 685), aggiungi:

```rust
    // wm.stay_awake(): il guest chiede di restare sveglio il PROSSIMO frame.
    // Azzerato dal run loop prima di ogni frame_all → va richiamato ogni frame
    // (modello egui request_repaint) per un aggiornamento continuo.
    linker.func_wrap("wm", "stay_awake",
        |mut caller: Caller<'_, T>| { caller.data_mut().win().stay_awake_request = true; })?;
    // wm.wake_on_pty(idx): lega una risorsa PTY a questa finestra; idx<0 = slega.
    // Il compositor sveglia la finestra dormiente quando quel pair ha output.
    linker.func_wrap("wm", "wake_on_pty",
        |mut caller: Caller<'_, T>, idx: i32| { caller.data_mut().win().wake_pty = idx; })?;
```

- [ ] **Step 2: Compila il kernel**

Run:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/w/Work/GitHub/ruos/kernel && cargo build --release 2>&1 | tail -8'
```
Expected: `Finished`.

- [ ] **Step 3: Commit (+ changelog)**

Crea `CHANGELOG/372-26-06-09-wm-host-stay-awake.md`.
```bash
cd /mnt/w/Work/GitHub/ruos
git add kernel/src/wasm/wt/wm.rs CHANGELOG/372-26-06-09-wm-host-stay-awake.md
git commit -m "feat(wm): host fns stay_awake + wake_on_pty"
```

---

## Task 6: Release del PTY legato al reap della finestra

**Files:**
- Modify: `kernel/src/wasm/wt/wm.rs` (`fn reap(&mut self)` ~riga 1333)

- [ ] **Step 1: Leggi `reap` per individuare dove una finestra viene rimossa**

Run:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && sed -n "1333,1374p" kernel/src/wasm/wt/wm.rs'
```
Individua il punto in cui un `Window` viene scartato (es. `self.wins.remove(i)` o dove `!w.alive`/`close_requested` porta alla rimozione).

- [ ] **Step 2: Shutdown del pair legato prima di scartare la finestra**

Subito PRIMA che la finestra venga rimossa (mentre hai ancora accesso al `Window` `w` o all'indice), aggiungi:

```rust
            // SP-sleep lifecycle: se questa finestra aveva un PTY legato
            // (terminale), chiudilo deterministicamente — la shell legge EOF ed
            // esce, il pair torna Free. Sostituisce il reap-a-timeout del watchdog
            // per i pair LocalGui.
            let wp = w.store.data().win.wake_pty;
            if wp >= 0 {
                crate::pty::request_shutdown(wp as usize);
            }
```

(Adatta `w` al binding reale nel tuo `reap` — se itera per indice, usa `self.wins[i].store.data().win.wake_pty`. Inseriscilo nel ramo che effettivamente rimuove/teardown la finestra, una sola volta per finestra reap-ata.)

- [ ] **Step 3: Compila il kernel**

Run:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/w/Work/GitHub/ruos/kernel && cargo build --release 2>&1 | tail -8'
```
Expected: `Finished`.

- [ ] **Step 4: Commit (+ changelog)**

Crea `CHANGELOG/373-26-06-09-wm-reap-releases-pty.md`.
```bash
cd /mnt/w/Work/GitHub/ruos
git add kernel/src/wasm/wt/wm.rs CHANGELOG/373-26-06-09-wm-reap-releases-pty.md
git commit -m "feat(wm): closing a terminal window shuts down its bound PTY pair"
```

---

## Task 7: Bindings `ruos-window` + auto-bind PTY

**Files:**
- Modify: `ruos-desktop/crates/ruos-window/src/lib.rs` (extern `mod wm` ~righe 20-40; wrapper pubblici; `RuosTermIo` ~righe 75-92)

Questo task è nel **submodule** ruos-desktop (commit nel submodule, niente changelog del padre qui).

- [ ] **Step 1: Aggiungi le extern**

In `mod wm { extern "C" { ... } }` (`ruos-window/src/lib.rs` ~riga 39, prima della `}` di `extern`):

```rust
        pub fn stay_awake();        // wm.stay_awake
        pub fn wake_on_pty(idx: i32); // wm.wake_on_pty
```

- [ ] **Step 2: Wrapper safe `stay_awake`**

Accanto agli altri wrapper pubblici (es. dopo `pub fn poweroff()` ~riga 165):

```rust
/// Tieni sveglia QUESTA finestra il prossimo frame. Chiamala ad OGNI frame finché
/// vuoi aggiornamento continuo (clock, animazioni, monitor live): di default il
/// compositor addormenta le finestre idle e ne salta la `frame()`.
pub fn stay_awake() {
    unsafe { wm::stay_awake() }
}
```

- [ ] **Step 3: Auto-bind il PTY in `RuosTermIo`**

In `impl gui_core::platform::TermIo for RuosTermIo` (~riga 75), aggiorna `term_open` e `term_close`:

```rust
    fn term_open(&mut self) -> Option<gui_core::platform::TermHandle> {
        let h = unsafe { term::open() };
        if h < 0 {
            None
        } else {
            // Lega il pair così il compositor sveglia il terminale dormiente
            // quando la shell produce output (comando lungo, job async).
            unsafe { wm::wake_on_pty(h); }
            Some(h)
        }
    }
    fn term_close(&mut self, h: gui_core::platform::TermHandle) {
        unsafe { wm::wake_on_pty(-1); term::close(h); }
    }
```

- [ ] **Step 4: Compila le app del submodule**

Run:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/w/Work/GitHub/ruos/ruos-desktop && cargo build -p terminal-app -p system-app --target wasm32-wasip1 --release 2>&1 | tail -8'
```
Expected: `Finished`. (Le import non risolte non falliscono il compile wasm — si risolvono al link kernel; ma il codice Rust deve compilare.)

- [ ] **Step 5: Commit (submodule)**

```bash
cd /mnt/w/Work/GitHub/ruos/ruos-desktop
git add crates/ruos-window/src/lib.rs
git commit -m "feat(ruos-window): stay_awake + wake_on_pty bindings; auto-bind terminal PTY"
```

---

## Task 8: `system-app` aggiornamento continuo

**Files:**
- Modify: `ruos-desktop/apps/system-app/src/lib.rs` (la `frame()` esportata)

Submodule. Il System Monitor aggiorna telemetria live → deve restare sveglio.

- [ ] **Step 1: Leggi la `frame()` di system-app**

Run:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos/ruos-desktop && cat apps/system-app/src/lib.rs'
```
Individua la `#[no_mangle] pub extern "C" fn frame()`.

- [ ] **Step 2: Chiama `stay_awake()` in cima a `frame()`**

All'inizio del corpo di `frame()` (prima del `frame_once`/pump), aggiungi:

```rust
    // System Monitor mostra telemetria live: opt-out dallo sleep, aggiorna sempre.
    ruos_window::stay_awake();
```

- [ ] **Step 3: Compila**

Run:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/w/Work/GitHub/ruos/ruos-desktop && cargo build -p system-app --target wasm32-wasip1 --release 2>&1 | tail -8'
```
Expected: `Finished`.

- [ ] **Step 4: Commit (submodule)**

```bash
cd /mnt/w/Work/GitHub/ruos/ruos-desktop
git add apps/system-app/src/lib.rs
git commit -m "feat(system-app): stay_awake — keep live telemetry updating while idle"
```

---

## Task 9: Boot-check selftests

**Files:**
- Modify: `kernel/src/wasm/wt/wm.rs` (nuove `#[cfg(feature = "boot-checks")] pub fn *_self_test()`, sul pattern di `lifecycle_self_test`/`spc_self_test` ~righe 1895-2090)
- Modify: `kernel/src/wasm/wt/mod.rs` (wrapper `#[cfg(feature = "boot-checks")] pub fn run_*_demo()` ~righe 106-155)
- Modify: il sito che invoca i `run_*_demo` sotto boot-checks (cerca dove `run_lifecycle_demo()` è chiamato e asserito)

- [ ] **Step 1: Trova come i selftest esistenti vengono invocati + asseriti**

Run:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && grep -rn "run_lifecycle_demo\|run_spc_demo\|lifecycle_self_test" kernel/src'
```
Studia `wm::lifecycle_self_test` (wm.rs ~1895) e `spc_self_test`: costruiscono un `Compositor` headless con moduli embedded e guidano `frame_all`. Replica quello scaffold.

- [ ] **Step 2: Aggiungi i selftest in `wm.rs`**

Aggiungi (gated `boot-checks`) accanto agli altri selftest. Usa il `tick` del guest (bumpa via `wm.tick`) come prova che `frame()` ha girato; un'app dormiente non bumpa.

```rust
/// Boot self-test SP-sleep: una finestra senza eventi dorme dopo GRACE_FRAMES.
/// Ritorna (tick_dopo_grazia, tick_a_riposo): se uguali → ha dormito.
#[cfg(feature = "boot-checks")]
pub fn sleep_idle_self_test() -> (u32, u32) {
    let mut c = Compositor::headless_with_self_closing(); // riusa lo scaffold dei selftest esistenti
    // Drena init + qualche frame: framed_once + grace scadono.
    for _ in 0..(Compositor::GRACE_FRAMES + 4) { c.frame_no = c.frame_no.wrapping_add(1); c.frame_all(); }
    let t1 = c.wins.last().map(|w| w.store.data().win.tick).unwrap_or(0);
    // Altri frame senza eventi: deve restare dormiente (tick fermo).
    for _ in 0..5 { c.frame_no = c.frame_no.wrapping_add(1); c.frame_all(); }
    let t2 = c.wins.last().map(|w| w.store.data().win.tick).unwrap_or(0);
    (t1, t2)
}

/// Boot self-test SP-sleep: un evento in coda risveglia la finestra (tick avanza).
#[cfg(feature = "boot-checks")]
pub fn sleep_event_wakes_self_test() -> (u32, u32) {
    let mut c = Compositor::headless_with_self_closing();
    for _ in 0..(Compositor::GRACE_FRAMES + 4) { c.frame_no = c.frame_no.wrapping_add(1); c.frame_all(); }
    let before = c.wins.last().map(|w| w.store.data().win.tick).unwrap_or(0);
    // Inietta un evento nella coda dell'ultima finestra.
    if let Some(w) = c.wins.last_mut() {
        w.store.data_mut().win.events.push_back(crate::gfx::GfxEvt { kind: 0, p0: 0, p1: 0, p2: 0, p3: 0 });
    }
    c.frame_no = c.frame_no.wrapping_add(1); c.frame_all();
    let after = c.wins.last().map(|w| w.store.data().win.tick).unwrap_or(0);
    (before, after)
}
```

> NOTA scaffold: `Compositor::headless_with_self_closing()` è uno pseudonimo del modo in cui `lifecycle_self_test` costruisce il compositor headless con l'app self-closing (registry idx 2). USA LA STESSA costruzione di `lifecycle_self_test` (copiala), non inventare un costruttore nuovo. Se l'app self-closing chiude subito, per questi test usa l'app probe che NON si chiude (quella di `wasip1_probe_self_test`/`egui_demo_self_test`) così la finestra resta viva a sufficienza. Scegli l'app embedded che resta aperta e bumpa `tick` quando `frame()` gira (verifica quale lo fa leggendo i selftest esistenti); se nessuna bumpa `tick`, usa `committed`/`pixels.len()` come proxy "ha girato".

Adatta i campi di `GfxEvt` ai reali (leggi `struct GfxEvt` in `crate::gfx`):
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && grep -n "struct GfxEvt" -A8 kernel/src/gfx*.rs kernel/src/gfx/*.rs 2>/dev/null'
```

- [ ] **Step 3: Aggiungi i wrapper `run_*_demo` in `wt/mod.rs`**

Accanto a `run_lifecycle_demo` (~riga 116):
```rust
#[cfg(feature = "boot-checks")]
pub fn run_sleep_idle_demo() -> (u32, u32) { crate::wasm::wt::wm::sleep_idle_self_test() }

#[cfg(feature = "boot-checks")]
pub fn run_sleep_event_demo() -> (u32, u32) { crate::wasm::wt::wm::sleep_event_wakes_self_test() }
```

- [ ] **Step 4: Invoca + asserisci al sito boot-checks**

Nel punto dove `run_lifecycle_demo()` viene chiamato e il risultato loggato/asserito (trovato a Step 1), aggiungi le chiamate ai nuovi demo e asserisci:
- `sleep_idle`: `t1 == t2` (dormito) → logga `SLEEP-IDLE-OK`, altrimenti panica/loga FAIL.
- `sleep_event`: `after > before` (svegliato) → `SLEEP-WAKE-OK`.

Segui ESATTAMENTE il modo in cui gli altri demo loggano la loro stringa di OK (la `make run-test` cerca la stringa di successo complessiva; non rompere il formato).

- [ ] **Step 5: Build boot-checks + run-test**

Run:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/w/Work/GitHub/ruos && make iso CARGO_FEATURES=boot-checks 2>&1 | tail -15 && make run-test 2>&1 | tail -30'
```
Expected: il boot completa, i nuovi `SLEEP-IDLE-OK` / `SLEEP-WAKE-OK` compaiono nel log seriale, e `make run-test` asserisce la stringa di successo complessiva (nessun panic).

- [ ] **Step 6: Commit (+ changelog)**

Crea `CHANGELOG/374-26-06-09-wm-sleep-bootchecks.md`.
```bash
cd /mnt/w/Work/GitHub/ruos
git add kernel/src/wasm/wt/wm.rs kernel/src/wasm/wt/mod.rs CHANGELOG/374-26-06-09-wm-sleep-bootchecks.md <sito-invocazione>
git commit -m "test(wm): boot-checks for idle sleep + event wake"
```

---

## Task 10: Build ISO completa + verifica manuale + bump submodule

**Files:** nessuna modifica di codice; integrazione + changelog finale.

- [ ] **Step 1: Build ISO completa (release, senza boot-checks)**

Run:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'source $HOME/.cargo/env && cd /mnt/w/Work/GitHub/ruos && make iso 2>&1 | tail -15'
```
Expected: ISO assemblata senza errori (ricompila i `.cwasm` con i bindings/system-app nuovi del submodule).

- [ ] **Step 2: Verifica manuale (QEMU `make run`, o VBox/hardware)**

Run `make run` (display) e verifica:
1. Apri il terminale, lascialo fermo >6 min → **non** muore (prima: `[process exited]`). Premi un tasto → risponde subito.
2. Nel terminale idle: `sleep 300; echo done`. Dopo ~5 min "done" appare anche senza toccare nulla (il `wake_on_pty` sveglia la finestra dormiente).
3. Apri System Monitor, dagli focus a un'altra finestra → la telemetria continua ad aggiornarsi (`stay_awake`).
4. Più finestre aperte e desktop fermo → il carico del GUI core scende (le `frame()` idle sono saltate). Osserva via System Monitor o netconsole.
5. Hover del mouse su una finestra dormiente → si ridisegna (sveglia su hover).

Se un punto fallisce → debug prima di procedere (usa `make iso CARGO_FEATURES=netconsole` + `tools/netconsole-rx` per i log su hardware).

- [ ] **Step 3: Bump del pointer submodule nel padre + changelog finale**

Crea `CHANGELOG/375-26-06-09-app-sleep-integration.md` (riassunto feature + bump submodule ruos-desktop).
```bash
cd /mnt/w/Work/GitHub/ruos
git add ruos-desktop CHANGELOG/375-26-06-09-app-sleep-integration.md
git commit -m "feat(desktop): app sleep/wake lifecycle — bump ruos-desktop submodule"
```

- [ ] **Step 4: (Solo se l'utente lo chiede) push**

Push solo su richiesta esplicita, e via WSL interattivo (auth):
```bash
cd /mnt/w/Work/GitHub/ruos/ruos-desktop && git push origin main
cd /mnt/w/Work/GitHub/ruos && git push origin main
```

---

## Note finali

- **Ordine runtime**: le host fn kernel (Task 5) shippano nella stessa ISO delle app che le importano (Task 7/8) → nessun fallimento di instantiate per import mancante. Non buildare una ISO intermedia con le app nuove ma senza Task 5.
- **Golden rule** (submodule): `ruos-window` resta thin FFI; nessun import OS in `gui-core`. Le modifiche app sono solo chiamate ai bindings.
- **Borrow nel run loop**: usa sempre `let fno = self.frame_no;` prima di iterare `self.wins` in `frame_all` per non prendere in prestito `self` due volte.
- **Grace tuning**: `GRACE_FRAMES=6` è un default; se la verifica manuale mostra micro-stutter sul primo input, alzalo (es. 10). Non è una costante ABI, cambiarla è sicuro.
- **Kernel senza `cargo test`**: la copertura logica è `should_wake`/`should_reap` (pure) + i boot-checks di Task 9; non esiste un test host veloce per il kernel in questo repo.
