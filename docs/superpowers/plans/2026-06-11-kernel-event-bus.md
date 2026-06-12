# Kernel event bus + notifiche compositor (v1) — Piano di implementazione

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** bus pub/sub kernel→compositor (ring broadcast con cursori) + shutdown/reboot
differito annullabile + toast/modale renderizzati dal compositor.

**Architettura:** modulo `kevent` (ring statico 64 slot, publish IRQ-safe zero-alloc,
side-table nomi); `power.rs` guadagna `request_poweroff/reboot/cancel/pending` con
enforcement in un task embassy (non la UI); il compositor (`wm.rs`) drena gli eventi
nel suo loop e disegna toast (INFO/WARN) e modale (CRIT) col modulo `decor`.

**Tech stack:** Rust `no_std`, `IrqMutex` (kernel/src/sync), `heapless` (nuova dep),
embassy-executor (`spawn_on`), Wasmtime AOT compositor in `kernel/src/wasm/wt/wm.rs`.

**Spec di riferimento (AUTORITATIVA):** `docs/superpowers/specs/2026-06-11-kernel-event-bus-design.md`

**Regole vincolanti (CLAUDE.md):**
- **NIENTE commit/push** se non richiesto esplicitamente dall'utente. I checkpoint
  di fine task = build/test verdi, non commit. (I passi "Commit" classici sono
  sostituiti da passi "Verifica build".)
- Branch: il lavoro NON va fatto su `feat/usb-wifi-rtl8188eu` (ha WIP non
  committato di altro scope). Prima di iniziare creare/branchare su
  `feat/kernel-event-bus` da `main` (o chiedere all'utente se preferisce
  worktree separato).
- Build via WSL: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && <cmd>'`
- Changelog: una entry in `CHANGELOG/` a fine lavoro (Task 8). Controllare il
  numero più alto esistente prima di crearla.
- Ogni modifica a host fn app-facing aggiorna `docs/api/` NELLO STESSO task.

**Comandi di verifica (abbreviati nel seguito come `WSL: <cmd>`):**
```bash
# build kernel + tool + ISO completa
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make iso'
# boot headless + assert stringa di successo
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test'
# self-test in-boot
wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/MinimalOS/BasicOperatingSystem && make run-test CARGO_FEATURES=boot-checks'
```

---

### Task 1: modulo bus — `kernel/src/kevent.rs`

**Files:**
- Create: `kernel/src/kevent.rs`
- Modify: `kernel/Cargo.toml` (dep `heapless`)
- Modify: `kernel/src/main.rs` (dichiarazione modulo)
- Modify: `kernel/src/boot/phases/devices.rs:34` circa (hook self-test boot-checks)

- [ ] **Step 1: aggiungi la dipendenza `heapless`**

In `kernel/Cargo.toml`, sezione `[dependencies]` (dopo `spin = "0.9"` a riga 11):

```toml
heapless = "0.8"
```

(`heapless` è pure-`no_std`, zero feature richieste.)

- [ ] **Step 2: dichiara il modulo**

In `kernel/src/main.rs` trova il blocco delle dichiarazioni di modulo (cerca
`mod klog` / `mod power`) e aggiungi accanto:

```rust
pub mod kevent;
```

- [ ] **Step 3: crea `kernel/src/kevent.rs`**

Contenuto completo:

```rust
//! Kernel event bus — ring broadcast pub/sub kernel→compositor.
//!
//! Spec: docs/superpowers/specs/2026-06-11-kernel-event-bus-design.md
//!
//! Publish = scrivere uno slot del ring + incrementare il seq monotonico:
//! IRQ-safe (IrqMutex), zero alloc, mai bloccante. Ogni lettore tiene il
//! proprio cursore `last_seq` e rileva da solo i gap (eventi sovrascritti);
//! il bus non registra subscriber. In v1 l'unico lettore è il compositor.

use core::sync::atomic::{AtomicU64, Ordering};

pub const RING_LEN: usize = 64;

// Severity.
pub const SEV_INFO: u8 = 0;
pub const SEV_WARN: u8 = 1;
pub const SEV_CRIT: u8 = 2;

// Catalogo kind v1 — byte alto = categoria (0x00 meta-bus, 0x01 power,
// 0x02 app/risorse; 0x03 storage e 0x04 hotplug/net riservati fase 2).
// MAI riusare/ridefinire il payload di un kind esistente: kind nuovi = ID nuovi.
/// Sintetizzato LOCALMENTE dal lettore su gap (mai scritto nel ring).
pub const KIND_SUBSCRIBER_OVERFLOW: u16 = 0x0001;
/// Evento di prova (self-test boot-checks + builtin debug `kev-test`).
pub const KIND_TEST: u16 = 0x0002;
pub const KIND_SHUTDOWN_PENDING: u16 = 0x0101; // payload [countdown_sec, reason, 0, 0]
pub const KIND_REBOOT_PENDING: u16 = 0x0102;   // payload [countdown_sec, reason, 0, 0]
pub const KIND_POWER_CANCELLED: u16 = 0x0103;  // payload [0; 4]
pub const KIND_APP_CRASHED: u16 = 0x0201;      // payload [win_id, causa, 0, 0] + nome
pub const KIND_APP_FUEL_EXHAUSTED: u16 = 0x0202; // payload [pid, 0, 0, 0] + nome
pub const KIND_MEM_LOW: u16 = 0x0203;          // payload [frame_liberi, frame_totali, 0, 0]

// APP_CRASHED.causa
pub const CRASH_TRAP: u32 = 0;        // trap WASM (o proc_exit del guest)
pub const CRASH_WATCHDOG: u32 = 1;    // epoch watchdog deadline
pub const CRASH_SPAWN_FAILED: u32 = 2; // instantiate/_initialize falliti

/// Evento del bus. Struct fissa `repr(C)`, versionata implicitamente dal `kind`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct KEvent {
    pub seq: u64,          // monotonico globale, parte da 1 (0 = slot vuoto)
    pub kind: u16,
    pub severity: u8,
    pub _pad: u8,
    pub ts_ticks: u32,     // tick timer 100 Hz al momento del publish
    pub payload: [u32; 4], // semantica per-kind (vedi catalogo)
}

impl KEvent {
    pub const ZERO: KEvent =
        KEvent { seq: 0, kind: 0, severity: 0, _pad: 0, ts_ticks: 0, payload: [0; 4] };
}

/// Ring + side-table nomi sotto UN lock (consistenza slot↔nome). I nomi app
/// non entrano nel payload fisso: copia troncata, stesso indice dello slot.
struct Bus {
    ring: [KEvent; RING_LEN],
    names: [heapless::String<32>; RING_LEN],
}

const EMPTY_NAME: heapless::String<32> = heapless::String::new();

static BUS: crate::sync::IrqMutex<Bus> = crate::sync::IrqMutex::new(Bus {
    ring: [KEvent::ZERO; RING_LEN],
    names: [EMPTY_NAME; RING_LEN],
});
/// Seq dell'ULTIMO evento pubblicato (0 = nessuno). Incrementato SOTTO il lock
/// BUS (l'ordine dei seq = l'ordine di scrittura degli slot); la load lock-free
/// serve solo a `current_seq()`.
static SEQ: AtomicU64 = AtomicU64::new(0);

/// Pubblica un evento. IRQ-safe, zero alloc, mai blocca (il critical section è
/// una copy di 32 byte). Slot di scrittura = `seq % RING_LEN` (circolare).
pub fn publish(kind: u16, severity: u8, payload: [u32; 4]) {
    publish_inner(kind, severity, payload, None);
}

/// Come `publish`, con nome associato (copiato TRONCATO a 32 byte nella
/// side-table — mai allocato).
pub fn publish_named(kind: u16, severity: u8, payload: [u32; 4], name: &str) {
    publish_inner(kind, severity, payload, Some(name));
}

fn publish_inner(kind: u16, severity: u8, payload: [u32; 4], name: Option<&str>) {
    let mut bus = BUS.lock();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed) + 1; // primo evento = seq 1
    let slot = (seq % RING_LEN as u64) as usize;
    bus.ring[slot] = KEvent {
        seq,
        kind,
        severity,
        _pad: 0,
        ts_ticks: crate::timer::ticks() as u32,
        payload,
    };
    bus.names[slot].clear();
    if let Some(n) = name {
        // Troncamento UTF-8-safe: push char-per-char finché c'è spazio.
        for ch in n.chars() {
            if bus.names[slot].push(ch).is_err() {
                break;
            }
        }
    }
}

/// Lettura da cursore: copia in `out` gli eventi con `seq > last_seq` in
/// ordine, ritorna `(n_copiati, lost)`. `lost > 0` se il ring ha sovrascritto
/// eventi mai letti (gap = seq_globale − last_seq − RING_LEN, se positivo);
/// in quel caso il lettore sintetizza localmente SUBSCRIBER_OVERFLOW{lost}.
/// Se gli eventi pendenti superano `out.len()` si richiama con il cursore
/// avanzato (l'ultimo `seq` copiato).
pub fn read_since(last_seq: u64, out: &mut [KEvent]) -> (usize, u64) {
    if out.is_empty() {
        return (0, 0);
    }
    let bus = BUS.lock();
    let cur = SEQ.load(Ordering::Relaxed);
    if cur <= last_seq {
        return (0, 0);
    }
    // Seq più vecchio ancora presente nel ring.
    let oldest = cur.saturating_sub(RING_LEN as u64 - 1).max(1);
    let lost = if last_seq + 1 < oldest { oldest - last_seq - 1 } else { 0 };
    let mut n = 0;
    let mut s = core::cmp::max(last_seq + 1, oldest);
    while s <= cur && n < out.len() {
        out[n] = bus.ring[(s % RING_LEN as u64) as usize];
        n += 1;
        s += 1;
    }
    (n, lost)
}

/// Nome associato all'evento `seq` (side-table). `None` se lo slot è stato
/// sovrascritto da un evento più recente o se l'evento non aveva nome.
pub fn name_of(seq: u64) -> Option<heapless::String<32>> {
    if seq == 0 {
        return None;
    }
    let bus = BUS.lock();
    let slot = (seq % RING_LEN as u64) as usize;
    if bus.ring[slot].seq != seq || bus.names[slot].is_empty() {
        return None;
    }
    Some(bus.names[slot].clone())
}

/// Seq corrente (ultimo pubblicato; 0 = nessun evento). Un lettore nuovo parte
/// da qui per NON rivedere il backlog (es. gli eventi del self-test in-boot).
pub fn current_seq() -> u64 {
    SEQ.load(Ordering::Relaxed)
}

/// Self-test in-boot (CARGO_FEATURES=boot-checks): pubblica RING_LEN+6 eventi,
/// verifica ordine seq e che `read_since` da cursore vecchio riporti lost == 6.
/// Stampa `KEVENT_TEST: OK` / `KEVENT_TEST: FAIL ...` (pattern engine_test).
#[cfg(feature = "boot-checks")]
pub fn self_test() {
    let base = current_seq();
    for i in 0..(RING_LEN as u32 + 6) {
        publish(KIND_TEST, SEV_INFO, [i, 0, 0, 0]);
    }
    let mut out = [KEvent::ZERO; RING_LEN];
    let (n, lost) = read_since(base, &mut out);
    let mut ok = n == RING_LEN && lost == 6 && out[0].seq == base + 7;
    for w in 0..n.saturating_sub(1) {
        if out[w + 1].seq != out[w].seq + 1 {
            ok = false;
        }
    }
    if ok {
        crate::kprintln!("KEVENT_TEST: OK");
    } else {
        crate::kprintln!(
            "KEVENT_TEST: FAIL n={} lost={} first_seq={} base={}",
            n, lost, out[0].seq, base
        );
    }
}
```

- [ ] **Step 4: hook del self-test nel boot**

In `kernel/src/boot/phases/devices.rs` (leggere prima il file), subito DOPO la
chiamata esistente

```rust
#[cfg(feature = "boot-checks")]
crate::console::engine_test::run();
```

aggiungi:

```rust
#[cfg(feature = "boot-checks")]
crate::kevent::self_test();
```

- [ ] **Step 5: verifica build + self-test**

Run: `WSL: make run-test CARGO_FEATURES=boot-checks`
Expected: il boot-marker passa E l'output seriale contiene `KEVENT_TEST: OK`.
Poi `WSL: make run-test` (senza feature) per la non-regressione.

---

### Task 2: shutdown/reboot differito annullabile — `kernel/src/power.rs`

**Files:**
- Modify: `kernel/src/power.rs` (le `poweroff()`/`reboot()` esistenti restano
  INVARIATE — sono il colpo finale; si aggiunge lo strato `request_*`)

- [ ] **Step 1: aggiungi stato + API differita**

In `kernel/src/power.rs`, dopo gli `use` esistenti (riga 15) aggiungi:

```rust
use crate::sync::IrqMutex;

/// Countdown di default per le richieste differite da host fn GUI (spec v1).
pub const DEFAULT_COUNTDOWN_SEC: u32 = 10;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PendingKind { Poweroff, Reboot }

#[derive(Clone, Copy)]
struct Pending {
    kind: PendingKind,
    deadline_tick: u64,
}

static PENDING: IrqMutex<Option<Pending>> = IrqMutex::new(None);

/// Richiede uno spegnimento differito annullabile: pubblica SHUTDOWN_PENDING
/// e spawna il task di enforcement. Richiesta duplicata mentre un PENDING è
/// attivo = no-op. Ritorna subito (NON è mai divergente).
pub fn request_poweroff(countdown_sec: u32) {
    request(PendingKind::Poweroff, countdown_sec);
}

/// Twin di `request_poweroff` per il riavvio (REBOOT_PENDING).
pub fn request_reboot(countdown_sec: u32) {
    request(PendingKind::Reboot, countdown_sec);
}

fn request(kind: PendingKind, countdown_sec: u32) {
    let deadline = crate::timer::ticks() + countdown_sec as u64 * 100;
    {
        let mut p = PENDING.lock();
        if p.is_some() {
            return; // già pendente: no-op
        }
        *p = Some(Pending { kind, deadline_tick: deadline });
    }
    let ev = match kind {
        PendingKind::Poweroff => crate::kevent::KIND_SHUTDOWN_PENDING,
        PendingKind::Reboot => crate::kevent::KIND_REBOOT_PENDING,
    };
    // reason 0 = richiesta utente (unico in v1).
    crate::kevent::publish(ev, crate::kevent::SEV_CRIT, [countdown_sec, 0, 0, 0]);
    crate::binfo!("power", "{:?} pending in {}s", kind, countdown_sec);
    // L'ENFORCEMENT è il task, non la UI: lo spegnimento avviene anche
    // headless o con compositor morto. Spawn sul BSP (core 0).
    if crate::executor::spawn_on(0, power_enforce_task(deadline)).is_err() {
        // Pool esaurito (2 cancel+re-request nello stesso countdown): rifiuta
        // la richiesta — la macchina resta accesa, l'utente ritenta.
        *PENDING.lock() = None;
        crate::kevent::publish(crate::kevent::KIND_POWER_CANCELLED,
                               crate::kevent::SEV_INFO, [0; 4]);
        crate::bwarn!("power", "enforce task pool full: request dropped");
    }
}

/// Annulla la richiesta pendente (se c'è) e pubblica POWER_CANCELLED. Il task
/// di enforcement in volo troverà PENDING == None e terminerà senza spegnere.
pub fn cancel() {
    let was = PENDING.lock().take();
    if was.is_some() {
        crate::kevent::publish(crate::kevent::KIND_POWER_CANCELLED,
                               crate::kevent::SEV_INFO, [0; 4]);
        crate::binfo!("power", "pending shutdown/reboot cancelled");
    }
}

/// Richiesta pendente: `(kind, tick rimanenti)`. Fonte di verità per il
/// countdown del modale (il compositor NON conta da solo).
pub fn pending() -> Option<(PendingKind, u64)> {
    let p = (*PENDING.lock())?;
    Some((p.kind, p.deadline_tick.saturating_sub(crate::timer::ticks())))
}

/// Task di enforcement: dorme fino alla deadline; al risveglio spegne SOLO se
/// PENDING è ancora attivo E la deadline è la SUA (un cancel + nuova richiesta
/// = un altro task con un'altra deadline). pool_size 2 copre il caso
/// cancel→re-request mentre il task vecchio sta ancora dormendo.
#[embassy_executor::task(pool_size = 2)]
async fn power_enforce_task(deadline: u64) {
    loop {
        let now = crate::timer::ticks();
        if now >= deadline {
            break;
        }
        crate::executor::delay::Delay::ticks(deadline - now).await;
    }
    let p = *PENDING.lock();
    if let Some(p) = p {
        if p.deadline_tick == deadline {
            match p.kind {
                PendingKind::Poweroff => poweroff(),
                PendingKind::Reboot => reboot(),
            }
        }
    }
    // PENDING annullato o sostituito: termina senza spegnere.
}
```

Nota: `crate::executor::delay` è `pub` (usato già da `fiber.rs:333`);
`spawn_on` è `pub` (`kernel/src/executor/mod.rs:239`). Il modulo `power.rs`
non ha oggi alcun `use crate::...` — aggiungerli in testa come sopra.

- [ ] **Step 2: verifica build**

Run: `WSL: make iso`
Expected: build verde (warn ok, error no).

---

### Task 3: publish dai punti kernel (APP_CRASHED, APP_FUEL_EXHAUSTED, MEM_LOW)

**Files:**
- Modify: `kernel/src/wasm/wt/wm.rs:1550-1566` (frame error path) e
  `kernel/src/wasm/wt/wm.rs:1393-1406` (spawn error path)
- Modify: `kernel/src/wasm/fiber.rs:321-324` (out-of-fuel)
- Modify: `kernel/src/memory/frames.rs:200-202` e `:210-212` (MEM_LOW + isteresi)

- [ ] **Step 1: APP_CRASHED dal path d'errore di `frame()`**

In `kernel/src/wasm/wt/wm.rs`, dentro `frame_all()`, sostituisci il braccio
`Err(e)` (righe 1552-1565) con:

```rust
                    Err(e) => {
                        // Anche un proc_exit volontario arriva qui come trap —
                        // il log distingue "app crashata" da "mai partita";
                        // il marker WATCHDOG distingue il kill da deadline.
                        let causa = if matches!(e.downcast_ref::<wasmtime::Trap>(),
                                                Some(wasmtime::Trap::Interrupt)) {
                            crate::bwarn!("wm",
                                "frame() WATCHDOG (epoch deadline) win_id={} '{}': killed",
                                w.id, w.title);
                            crate::kevent::CRASH_WATCHDOG
                        } else {
                            crate::bwarn!("wm", "frame() err win_id={}: {:?}", w.id, e);
                            crate::kevent::CRASH_TRAP
                        };
                        crate::kevent::publish_named(crate::kevent::KIND_APP_CRASHED,
                            crate::kevent::SEV_WARN, [w.id, causa, 0, 0], &w.title);
                        w.store.data_mut().win.close_requested = true;
                    }
```

- [ ] **Step 2: APP_CRASHED (causa spawn-failed) dai due error path di `spawn_named`**

Sempre in `wm.rs`, nel path instantiate-fallita (righe 1395-1399), dopo la
`bwarn!` esistente aggiungi:

```rust
                crate::kevent::publish_named(crate::kevent::KIND_APP_CRASHED,
                    crate::kevent::SEV_WARN,
                    [id, crate::kevent::CRASH_SPAWN_FAILED, 0, 0], name);
```

e identicamente nel path `_initialize`-fallita (righe 1402-1406), dopo la sua
`bwarn!`:

```rust
            crate::kevent::publish_named(crate::kevent::KIND_APP_CRASHED,
                crate::kevent::SEV_WARN,
                [id, crate::kevent::CRASH_SPAWN_FAILED, 0, 0], name);
```

(`id` e `name` sono già in scope in entrambi i punti.)

- [ ] **Step 3: APP_FUEL_EXHAUSTED dal runtime wasmi**

In `kernel/src/wasm/fiber.rs`, sostituisci il braccio (righe 321-324):

```rust
                ResumableCall::OutOfFuel(_) => {
                    kprintln!("wasm: task killed (fuel exhausted)");
                    return 137;
                }
```

con:

```rust
                ResumableCall::OutOfFuel(_) => {
                    kprintln!("wasm: task killed (fuel exhausted)");
                    let pid = self.pid.unwrap_or(0);
                    // Nome dal proc-registry (evento raro: la lookup alloca, ok —
                    // siamo in contesto fiber/executor, non IRQ).
                    let name = self.pid.and_then(|pid| {
                        crate::proc::list().into_iter()
                            .find(|p| p.pid == pid)
                            .map(|p| p.name)
                    });
                    match name {
                        Some(n) => crate::kevent::publish_named(
                            crate::kevent::KIND_APP_FUEL_EXHAUSTED,
                            crate::kevent::SEV_WARN, [pid, 0, 0, 0], &n),
                        None => crate::kevent::publish(
                            crate::kevent::KIND_APP_FUEL_EXHAUSTED,
                            crate::kevent::SEV_WARN, [pid, 0, 0, 0]),
                    }
                    return 137;
                }
```

- [ ] **Step 4: MEM_LOW con isteresi dal frame allocator**

In `kernel/src/memory/frames.rs`: leggere prima `FrameCounts` (riga 24) per i
tipi esatti dei campi (`total`/`used`/`free`); il codice sotto usa cast `as u64`
quindi funziona sia con `u64` sia con `usize`. In testa al file aggiungi agli
`use` esistenti `core::sync::atomic::{AtomicBool, Ordering}` (o estendi quello
già presente), poi vicino alle fn wrapper module-level (riga ~200):

```rust
/// MEM_LOW: soglia frame liberi < 10% del totale, con isteresi — dopo il
/// publish si ri-arma solo quando i liberi risalgono sopra il 15%. Un evento
/// per attraversamento, niente spam.
static MEM_LOW_ARMED: AtomicBool = AtomicBool::new(true);

fn mem_low_check(c: FrameCounts) {
    let (free, total) = (c.free as u64, c.total as u64);
    if total == 0 {
        return;
    }
    if MEM_LOW_ARMED.load(Ordering::Relaxed) {
        if free * 10 < total {
            MEM_LOW_ARMED.store(false, Ordering::Relaxed);
            crate::kevent::publish(crate::kevent::KIND_MEM_LOW,
                crate::kevent::SEV_WARN, [free as u32, total as u32, 0, 0]);
        }
    } else if free * 100 > total * 15 {
        MEM_LOW_ARMED.store(true, Ordering::Relaxed);
    }
}
```

e modifica i due allocatori wrapper perché chiamino il check FUORI dal lock
`FRAMES` (publish prende il suo IrqMutex; mai annidarlo dentro il lock
dell'allocatore):

```rust
pub fn allocate_frame() -> Option<PhysFrame<Size4KiB>> {
    let (r, counts) = {
        let mut g = FRAMES.lock();
        match g.as_mut() {
            Some(f) => {
                let r = f.allocate_frame();
                let c = f.counts();
                (r, Some(c))
            }
            None => (None, None),
        }
    };
    if let Some(c) = counts {
        mem_low_check(c);
    }
    r
}
```

```rust
pub fn allocate_contiguous(n: u64) -> Option<PhysFrame<Size4KiB>> {
    let (r, counts) = {
        let mut g = FRAMES.lock();
        match g.as_mut() {
            Some(f) => {
                let r = f.allocate_contiguous(n);
                let c = f.counts();
                (r, Some(c))
            }
            None => (None, None),
        }
    };
    if let Some(c) = counts {
        mem_low_check(c);
    }
    r
}
```

- [ ] **Step 5: verifica build + non-regressione**

Run: `WSL: make run-test`
Expected: PASS (i publish sono inerti finché nessuno legge).

---

### Task 4: cambio semantica ABI `wm.poweroff` / `wm.reboot` (+ docs STESSO task)

**Files:**
- Modify: `kernel/src/wasm/wt/wm.rs:848-859` (func_wrap)
- Modify: `docs/api/wm.md:11` (Last reviewed) e `:132-136` (entry)
- Modify: `docs/api/ruos-window.md:121` (riga tabella poweroff)
- Modify: `ruos-desktop/crates/ruos-window/src/lib.rs:31-32` e `:173-181` (commenti — submodule!)

- [ ] **Step 1: host fn → richiesta differita**

In `kernel/src/wasm/wt/wm.rs` sostituisci le righe 848-859 con:

```rust
    // wm.poweroff(): the calling window (the shell's power button) asks for a
    // DEFERRED, CANCELLABLE poweroff: publishes SHUTDOWN_PENDING on the kevent
    // bus (the compositor shows the countdown modal) and RETURNS to the guest.
    // Enforcement = power_enforce_task (fires even if the GUI dies). The old
    // immediate-never-return semantics moved to `crate::power::poweroff()`.
    linker.func_wrap("wm", "poweroff",
        |_caller: Caller<'_, T>| {
            crate::power::request_poweroff(crate::power::DEFAULT_COUNTDOWN_SEC);
        })?;
    // wm.reboot(): twin of wm.poweroff — deferred, cancellable REBOOT_PENDING.
    linker.func_wrap("wm", "reboot",
        |_caller: Caller<'_, T>| {
            crate::power::request_reboot(crate::power::DEFAULT_COUNTDOWN_SEC);
        })?;
```

NB: NON toccare `ruos:gui/power` in `gui.rs` né il mondo bringup in
`component.rs` — restano immediati (la spec cambia solo `wm.*`; il gate
component-model USA il never-return per i boot-check).

- [ ] **Step 2: `docs/api/wm.md`**

Sostituisci le entry (righe 132-136):

```markdown
### `poweroff()`
Request a deferred poweroff: returns immediately; the kernel powers off after
10 s unless cancelled (the compositor shows a countdown modal with a Cancel
button / Esc). Calling it again while a request is pending is a no-op.

### `reboot()`
Twin of `poweroff()` for restart: deferred 10 s, cancellable from the modal.
```

e aggiorna la riga 11:

```markdown
**Last reviewed:** 2026-06-11 (22 functions; `poweroff()`/`reboot()` are now deferred + cancellable).
```

- [ ] **Step 3: `docs/api/ruos-window.md` riga 121**

Sostituisci la riga della tabella:

```markdown
| `poweroff()` | Request deferred poweroff (10 s, cancellable from the compositor modal). |
```

(Se la tabella ha anche una riga `reboot()`, aggiornala allo stesso modo.)
Aggiorna il "Last reviewed" della pagina se presente.

- [ ] **Step 4: commenti SDK `ruos-window` (submodule `ruos-desktop`)**

In `ruos-desktop/crates/ruos-window/src/lib.rs`:

righe 31-32, nuovi commenti:

```rust
        pub fn poweroff(); // wm.poweroff (DEFERRED power off: 10 s, cancellable; returns)
        pub fn reboot(); // wm.reboot (DEFERRED restart: 10 s, cancellable; returns)
```

righe 173-181, nuovi doc-comment:

```rust
/// Request a deferred power off (the shell's power button): the kernel shows a
/// 10 s countdown modal (cancellable) and powers off when it expires. Returns.
pub fn poweroff() {
    unsafe { wm::poweroff() }
}

/// Request a deferred restart (the shell's reboot button). Twin of `poweroff`.
pub fn reboot() {
    unsafe { wm::reboot() }
}
```

NB: è un SUBMODULE — le modifiche vivono in `ruos-desktop` (repo separato).
Niente commit (regola CLAUDE.md); segnalare nel riepilogo finale che il
submodule ha modifiche locali.

- [ ] **Step 5: verifica build**

Run: `WSL: make iso`
Expected: verde (il cambiamento è solo kernel-side: le firme extern del guest
non cambiano, quindi i `.cwasm` esistenti restano validi).

---

### Task 5: compositor — drain eventi, toast, modale

**Files:**
- Modify: `kernel/src/wasm/wt/wm.rs` — modulo `decor` (riga ~609), struct
  `Compositor` (riga ~1045), i DUE costruttori (righe ~1105 e ~1150), il loop
  `run()` (input ~1779-1860, present-gate ~1984), `present()` (~1666).

- [ ] **Step 1: `decor::draw_text_at` (testo SENZA centratura verticale)**

`decor::draw_text` centra verticalmente dentro `buf_h` — col backbuf
full-screen finirebbe a metà schermo. Aggiungi nel modulo `decor` (dopo
`draw_text`, riga ~671):

```rust
    /// Come `draw_text` ma SENZA centratura verticale dentro `buf_h`: il pen
    /// parte esattamente da (x, y) buffer-local. Serve agli overlay del
    /// compositor (toast/modale), disegnati nel back-buffer full-screen.
    pub fn draw_text_at(buf: &mut [u8], buf_w: u32, buf_h: u32,
                        x: u32, y: u32, max_x: u32, text: &str, c: [u8; 4]) {
        let gw = crate::console::font::glyph_width() as u32;
        let mut pen = x;
        for ch in text.chars() {
            if pen + gw > max_x { break; }
            let r = crate::console::font::raster_for_weight(ch, false);
            blend_glyph(buf, buf_w, buf_h, pen as i32, y as i32, r.raster(), c);
            pen += gw;
        }
    }
```

- [ ] **Step 2: tipi + costanti overlay**

Vicino a `WORKAREA_TOP` (riga ~1076) aggiungi:

```rust
/// Overlay notifiche (spec kernel-event-bus v1).
const TOAST_W: u32 = 260;
const TOAST_H: u32 = 36;
const TOAST_PAD: u32 = 8;
const TOAST_LIFE_TICKS: u64 = 500; // ~5 s @ 100 Hz
const TOAST_MAX_VISIBLE: usize = 3;

/// Una notifica toast (alloc lecita: contesto compositor, non IRQ).
pub struct Toast {
    pub text: alloc::string::String,
    pub sev: u8,
    /// None finché il toast non entra fra i TOAST_MAX_VISIBLE (coda FIFO);
    /// la vita (TOAST_LIFE_TICKS) parte da quando diventa visibile.
    pub born_tick: Option<u64>,
}

/// Modale CRIT shutdown/reboot. La fonte di verità del countdown è
/// `power::pending()` — qui solo lo stato di ridisegno (ultimo secondo visto).
pub struct PowerModal {
    pub last_secs: u64,
}
```

- [ ] **Step 3: campi `Compositor` + init nei DUE costruttori**

Alla struct `Compositor` (dopo `frame_deadline_override`, riga ~1065) aggiungi:

```rust
    /// Cursore di lettura sul kevent bus (seq dell'ultimo evento consumato).
    kev_cursor: u64,
    /// Toast attivi: i primi TOAST_MAX_VISIBLE sono a schermo, il resto in coda.
    toasts: alloc::collections::VecDeque<Toast>,
    /// Modale shutdown/reboot attivo (Some = input routato SOLO al modale).
    modal: Option<PowerModal>,
```

In ENTRAMBI gli struct-literal dei costruttori (righe ~1105-1117 e ~1150-1162),
dopo `frame_deadline_override: None,` aggiungi:

```rust
            // Parte dal seq corrente: NON ripresenta il backlog di boot
            // (es. gli eventi del self-test boot-checks) come toast.
            kev_cursor: crate::kevent::current_seq(),
            toasts: alloc::collections::VecDeque::new(),
            modal: None,
```

- [ ] **Step 4: metodi drain/toast/modale**

Aggiungi questi metodi a `impl Compositor` (dopo `on_left_down`, riga ~1725):

```rust
    /// Drena il kevent bus (nuovo step del run loop, dopo la fase input):
    /// `read_since(cursor)` → smista per severity (toast INFO/WARN, modale CRIT).
    fn drain_kevents(&mut self) {
        let mut buf = [crate::kevent::KEvent::ZERO; 16];
        loop {
            let (n, lost) = crate::kevent::read_since(self.kev_cursor, &mut buf);
            if lost > 0 {
                // Gap: il ring ha sovrascritto eventi mai letti →
                // SUBSCRIBER_OVERFLOW sintetizzato localmente (mai nel ring).
                self.push_toast(
                    alloc::format!("bus eventi: persi {} eventi", lost),
                    crate::kevent::SEV_INFO);
            }
            if n == 0 {
                break;
            }
            self.kev_cursor = buf[n - 1].seq;
            for i in 0..n {
                let ev = buf[i];
                self.handle_kevent(&ev);
            }
            if n < buf.len() {
                break;
            }
        }
    }

    fn handle_kevent(&mut self, ev: &crate::kevent::KEvent) {
        use crate::kevent as kev;
        match ev.kind {
            kev::KIND_SHUTDOWN_PENDING | kev::KIND_REBOOT_PENDING => {
                self.modal = Some(PowerModal { last_secs: 0 });
                self.dirty = true;
            }
            kev::KIND_POWER_CANCELLED => {
                if self.modal.take().is_some() {
                    self.dirty = true;
                }
            }
            kev::KIND_APP_CRASHED => {
                let name = kev::name_of(ev.seq);
                let causa = match ev.payload[1] {
                    kev::CRASH_WATCHDOG => "watchdog",
                    kev::CRASH_SPAWN_FAILED => "avvio fallito",
                    _ => "crash",
                };
                let text = match &name {
                    Some(n) => alloc::format!("app '{}' terminata ({})", n.as_str(), causa),
                    None => alloc::format!("app win_id={} terminata ({})", ev.payload[0], causa),
                };
                self.push_toast(text, ev.severity);
            }
            kev::KIND_APP_FUEL_EXHAUSTED => {
                let name = kev::name_of(ev.seq);
                let text = match &name {
                    Some(n) => alloc::format!("'{}' fermata: fuel esaurito", n.as_str()),
                    None => alloc::format!("pid {}: fuel esaurito", ev.payload[0]),
                };
                self.push_toast(text, ev.severity);
            }
            kev::KIND_MEM_LOW => {
                self.push_toast(
                    alloc::format!("memoria quasi esaurita: {}/{} frame liberi",
                                   ev.payload[0], ev.payload[1]),
                    ev.severity);
            }
            kev::KIND_TEST => {
                let name = kev::name_of(ev.seq);
                self.push_toast(
                    alloc::format!("evento di test ({})", name.as_deref().unwrap_or("?")),
                    ev.severity);
            }
            _ => {
                // Kind sconosciuto (catalogo futuro): toast generico, mai drop silente.
                self.push_toast(
                    alloc::format!("kevent kind={:#06x}", ev.kind),
                    ev.severity);
            }
        }
    }

    fn push_toast(&mut self, text: alloc::string::String, sev: u8) {
        self.toasts.push_back(Toast { text, sev, born_tick: None });
        self.dirty = true;
    }

    /// Promozione FIFO + scadenza (~5 s da quando un toast diventa visibile).
    fn tick_toasts(&mut self) {
        let now = crate::timer::ticks();
        for t in self.toasts.iter_mut().take(TOAST_MAX_VISIBLE) {
            if t.born_tick.is_none() {
                t.born_tick = Some(now);
                self.dirty = true;
            }
        }
        let before = self.toasts.len();
        self.toasts.retain(|t| match t.born_tick {
            Some(b) => now.saturating_sub(b) < TOAST_LIFE_TICKS,
            None => true,
        });
        if self.toasts.len() != before {
            self.dirty = true;
        }
    }

    /// Sincronizza il modale con `power::pending()` (fonte di verità) e
    /// ridisegna a ogni cambio di secondo del countdown.
    fn tick_modal(&mut self) {
        if self.modal.is_none() {
            return;
        }
        match crate::power::pending() {
            None => {
                // Annullato altrove (kev-test cancel) o già spento: chiudi.
                self.modal = None;
                self.dirty = true;
            }
            Some((_, remaining)) => {
                let secs = remaining / 100 + 1;
                if self.modal.as_ref().map(|m| m.last_secs) != Some(secs) {
                    if let Some(m) = self.modal.as_mut() {
                        m.last_secs = secs;
                    }
                    self.dirty = true;
                }
            }
        }
    }

    /// Geometria modale: rect centrato + bottone Annulla. Calcolata on-the-fly
    /// (stessa fn per draw e hit-test, mai disallineate).
    fn modal_rects(sw: u32, sh: u32) -> ((u32, u32, u32, u32), (u32, u32, u32, u32)) {
        const MW: u32 = 360;
        const MH: u32 = 120;
        let mx = sw.saturating_sub(MW) / 2;
        let my = sh.saturating_sub(MH) / 2;
        const BW: u32 = 100;
        const BH: u32 = 28;
        let bx = mx + (MW - BW) / 2;
        let by = my + MH - BH - 12;
        ((mx, my, MW, MH), (bx, by, BW, BH))
    }

    /// Hit-test dei toast visibili (stessa geometria di `draw_overlays`).
    fn toast_at(&self, px: i32, py: i32) -> Option<usize> {
        let g = crate::gfx::geom();
        if g.width == 0 {
            return None;
        }
        let x = g.width.saturating_sub(TOAST_W + TOAST_PAD) as i32;
        let mut y = (WORKAREA_TOP + TOAST_PAD) as i32;
        for i in 0..self.toasts.len().min(TOAST_MAX_VISIBLE) {
            if px >= x && px < x + TOAST_W as i32 && py >= y && py < y + TOAST_H as i32 {
                return Some(i);
            }
            y += (TOAST_H + TOAST_PAD) as i32;
        }
        None
    }

    /// Input mentre il modale è attivo: TUTTO routato qui, niente alle finestre.
    /// Esc (scancode set-1 0x01) o click su Annulla → `power::cancel()`; il
    /// modale si chiude via POWER_CANCELLED / `pending() == None` (tick_modal).
    fn modal_input(&mut self, ev: &crate::gfx::GfxEvt, cx: i32, cy: i32) {
        match ev.kind {
            0 => {
                // key: p0 = scancode PS/2 set-1, p1 = pressed.
                if ev.p0 == 0x01 && ev.p1 != 0 {
                    crate::power::cancel();
                }
            }
            2 => {
                // left press dentro il bottone Annulla.
                if ev.p0 == 0 && ev.p1 != 0 {
                    let g = crate::gfx::geom();
                    let (_, (bx, by, bw, bh)) = Self::modal_rects(g.width, g.height);
                    if cx >= bx as i32 && cx < (bx + bw) as i32
                        && cy >= by as i32 && cy < (by + bh) as i32 {
                        crate::power::cancel();
                    }
                }
            }
            _ => {}
        }
    }

    /// Disegna toast + modale nel back-buffer, DOPO il composite delle finestre
    /// e PRIMA del blit (il cursore software è ricomposto dal blit → resta sopra).
    fn draw_overlays(&mut self, sw: u32, sh: u32) {
        let gh = crate::console::font::glyph_height() as u32;
        // -- toast: stack in alto a destra, bordo per severity --
        {
            let buf = &mut self.backbuf[..];
            let x = sw.saturating_sub(TOAST_W + TOAST_PAD);
            let mut y = WORKAREA_TOP + TOAST_PAD;
            for t in self.toasts.iter().take(TOAST_MAX_VISIBLE) {
                let border = if t.sev >= crate::kevent::SEV_WARN {
                    [0xE0, 0xA0, 0x20, 0xFF] // ambra (WARN)
                } else {
                    [0x80, 0x80, 0x80, 0xFF] // grigio (INFO)
                };
                decor::fill_rect(buf, sw, sh, x, y, TOAST_W, TOAST_H, border);
                decor::fill_rect(buf, sw, sh, x + 2, y + 2, TOAST_W - 4, TOAST_H - 4,
                                 [0x20, 0x28, 0x30, 0xFF]);
                decor::draw_text_at(buf, sw, sh,
                    x + 8, y + TOAST_H.saturating_sub(gh) / 2, x + TOAST_W - 8,
                    &t.text, [0xF0, 0xF0, 0xF0, 0xFF]);
                y += TOAST_H + TOAST_PAD;
            }
        }
        // -- modale CRIT (countdown letto da power::pending(), non dall'evento) --
        if self.modal.is_some() {
            if let Some((kind, remaining)) = crate::power::pending() {
                let buf = &mut self.backbuf[..];
                let ((mx, my, mw, mh), (bx, by, bw, bh)) = Self::modal_rects(sw, sh);
                let secs = remaining / 100 + 1;
                decor::fill_rect(buf, sw, sh, mx, my, mw, mh, [0xC0, 0x30, 0x30, 0xFF]);
                decor::fill_rect(buf, sw, sh, mx + 2, my + 2, mw - 4, mh - 4,
                                 [0x18, 0x20, 0x28, 0xFF]);
                let title = match kind {
                    crate::power::PendingKind::Poweroff => "Spegnimento",
                    crate::power::PendingKind::Reboot => "Riavvio",
                };
                decor::draw_text_at(buf, sw, sh, mx + 16, my + 14, mx + mw - 16,
                                    title, [0xFF, 0xFF, 0xFF, 0xFF]);
                let line = alloc::format!("tra {} s  (Esc per annullare)", secs);
                decor::draw_text_at(buf, sw, sh, mx + 16, my + 14 + gh + 8, mx + mw - 16,
                                    &line, [0xE0, 0xE0, 0xE0, 0xFF]);
                decor::fill_rect(buf, sw, sh, bx, by, bw, bh, [0x40, 0x50, 0x60, 0xFF]);
                decor::draw_text_at(buf, sw, sh,
                    bx + 18, by + bh.saturating_sub(gh) / 2, bx + bw,
                    "Annulla", [0xFF, 0xFF, 0xFF, 0xFF]);
            }
        }
    }
```

- [ ] **Step 5: routing input — modale prima di tutto, poi hit-test toast**

In `run()`, dentro `while let Some(ev) = crate::gfx::pop()` (riga ~1779),
SUBITO dopo `let (cx, cy) = crate::gfx::mouse_pos();` e PRIMA del
`match ev.kind {`, inserisci:

```rust
                // Modale attivo: input mouse/tastiera routato SOLO al modale,
                // niente alle finestre (Esc / click su Annulla → power::cancel()).
                if self.modal.is_some() {
                    self.modal_input(&ev, cx, cy);
                    continue;
                }
                // Click su un toast = dismiss immediato. Hit-test PRIMA di
                // quello finestre. La press è consumata (btn_l resta false →
                // la release sotto non viene inoltrata a nessuna finestra).
                if ev.kind == 2 && ev.p0 == 0 && ev.p1 != 0 {
                    if let Some(i) = self.toast_at(cx, cy) {
                        self.toasts.remove(i);
                        self.dirty = true;
                        continue;
                    }
                }
```

- [ ] **Step 6: step di drain nel loop**

Sempre in `run()`, subito DOPO la chiusura del `while let Some(ev) ... { }`
(riga ~1860) e PRIMA del blocco "Clear per-window damage flags", inserisci:

```rust
            // Notifiche kernel (spec kernel-event-bus): drena il bus, promuovi/
            // scadi i toast, sincronizza il modale col PENDING di power.
            self.drain_kevents();
            self.tick_toasts();
            self.tick_modal();
```

- [ ] **Step 7: disegno overlay in `present()`**

In `present()` (riga ~1666), tra `dispatch_bands(...)` e `crate::gfx::blit(...)`:

```rust
        dispatch_bands(back_ptr, stride, sw, sh, bg, n);
        // Overlay notifiche: sopra le finestre composite, sotto il cursore
        // software (che è ricomposto dal blit).
        self.draw_overlays(sw, sh);
        crate::gfx::blit(&self.backbuf[..needed], 0, 0, sw, sh);
```

- [ ] **Step 8: verifica build + boot**

Run: `WSL: make iso && make run-test`
Expected: PASS. (La GUI non è esercitata da run-test; la verifica visiva è in
Task 7.)

---

### Task 6: builtin debug `kev-test` (host fn wasmi + shell + docs)

**Files:**
- Modify: `kernel/src/wasm/host/proc.rs` (host fn + registrazione, righe ~555 e ~958-981)
- Modify: `user/shell/src/main.rs` (extern + builtin + help + is_builtin, righe ~6-13, ~399, ~435, ~612)
- Modify: `docs/api/ruos.md` (nuova entry + Last reviewed)

- [ ] **Step 1: host fn `ruos.kev_test`**

In `kernel/src/wasm/host/proc.rs`, dopo `ruos_reboot` (riga ~564):

```rust
/// ruos_kev_test(mode) → 0 ok, -1 modo sconosciuto. DEBUG del kevent bus
/// (builtin shell `kev-test`): 0 = pubblica un evento WARN di prova (toast),
/// 1 = request_poweroff differito, 2 = request_reboot differito, 3 = cancel.
/// Innocuo (equivale a poteri che `ruos.poweroff` già concede); rimovibile.
pub fn ruos_kev_test(_caller: Caller<'_, RuntimeState>, mode: i32) -> Result<i32, Error> {
    match mode {
        0 => {
            crate::kevent::publish_named(crate::kevent::KIND_TEST,
                crate::kevent::SEV_WARN, [42, 0, 0, 0], "kev-test");
            Ok(0)
        }
        1 => { crate::power::request_poweroff(crate::power::DEFAULT_COUNTDOWN_SEC); Ok(0) }
        2 => { crate::power::request_reboot(crate::power::DEFAULT_COUNTDOWN_SEC); Ok(0) }
        3 => { crate::power::cancel(); Ok(0) }
        _ => Ok(-1),
    }
}
```

e nella `pub fn link(...)` (riga ~958) aggiungi alla catena:

```rust
        .func_wrap("ruos", "kev_test", ruos_kev_test)?
```

- [ ] **Step 2: builtin shell**

In `user/shell/src/main.rs`:

(a) nell'`extern "C"` del modulo `ruos` (righe 6-13) aggiungi:

```rust
    fn kev_test(mode: i32) -> i32;
```

(b) nel `match argv[0]` (riga ~399) aggiungi il braccio (prima del catch-all
`cmd =>`):

```rust
        "kev-test" => builtin_kev_test(&argv),
```

(c) vicino a `builtin_pwd` (riga ~435) aggiungi (adattare il tipo di `argv` a
quello degli altri builtin, es. `&[&str]` o `&[String]` — copiare la firma di
`builtin_cd`):

```rust
fn builtin_kev_test(argv: &[&str]) {
    let mode = match argv.get(1).map(|s| &**s) {
        None | Some("toast") => 0,
        Some("poweroff") => 1,
        Some("reboot") => 2,
        Some("cancel") => 3,
        Some(other) => {
            println!("kev-test: modo sconosciuto '{}' (toast|poweroff|reboot|cancel)", other);
            return;
        }
    };
    let r = unsafe { kev_test(mode) };
    if r != 0 {
        println!("kev-test: errore {}", r);
    }
}
```

(d) in `is_builtin` (riga ~612) aggiungi `"kev-test"` alla `matches!`;
(e) in `builtin_help` aggiungi una riga `kev-test [toast|poweroff|reboot|cancel]`.

- [ ] **Step 3: documenta in `docs/api/ruos.md` (STESSO task — regola CLAUDE.md)**

Aggiungi una entry (sezione misc/debug, vicino a `poweroff()`/`reboot()` riga ~151):

```markdown
### `kev_test(mode: i32) -> i32`
DEBUG del kernel event bus (usata dal builtin `kev-test` della shell). `mode`:
`0` pubblica un evento WARN di prova (toast nel compositor), `1` richiede il
poweroff differito (10 s, annullabile), `2` il reboot differito, `3` annulla la
richiesta pendente. Ritorna `0`, o `-1` per modo sconosciuto. Nota: `ruos.poweroff`
/`ruos.reboot` (sopra) restano IMMEDIATI e mai-ritornanti.
```

e aggiorna il "Last reviewed" della pagina a 2026-06-11.

- [ ] **Step 4: verifica build**

Run: `WSL: make iso`
Expected: verde (shell.wasm ricompilata col nuovo import; l'host fn è sempre
registrata quindi l'instantiate non può fallire).

---

### Task 7: verifica end-to-end

- [ ] **Step 1: regressione**

Run: `WSL: make run-test` e `WSL: make run-test CARGO_FEATURES=boot-checks`
Expected: entrambi PASS; il secondo stampa `KEVENT_TEST: OK`.

- [ ] **Step 2: verifica visiva toast (QEMU con display)**

Run: `WSL: make run` (da terminale interattivo; se l'ambiente di esecuzione non
ha display, delegare all'utente questo step e i successivi).
Nella shell: `kev-test` → nella console nulla (niente compositor); poi
`compositor` per entrare nel desktop e di nuovo da un terminale GUI o via SSH:
`kev-test` → toast WARN bordo ambra in alto a destra, sparisce dopo ~5 s;
click su un toast = dismiss immediato.

- [ ] **Step 3: verifica modale + Annulla + spegnimento**

Nel desktop: click sul bottone power della shell (o `kev-test poweroff` via
SSH) → modale centrato "Spegnimento / tra N s" con countdown che scala ogni
secondo; le finestre NON ricevono input; `Esc` (o click su Annulla) → il modale
si chiude, il desktop torna interattivo. Ripetere e lasciar scadere → QEMU si
spegne.

- [ ] **Step 4: caso negativo — enforcement headless**

Run: `WSL: make run` SENZA entrare nel compositor; nella shell `kev-test poweroff`
→ dopo 10 s la macchina si spegne comunque (QEMU esce). Conferma che
l'enforcement è il task async, non la UI.

---

### Task 8: changelog + stato spec

- [ ] **Step 1: entry changelog**

Controlla il numero più alto in `CHANGELOG/` (al 2026-06-11 le entry recenti
sono ~463 — usare il successivo libero, contatore a progressione +1, mai
riusato). Crea `CHANGELOG/NN-26-06-11-kernel-event-bus.md`:

```markdown
# NN — Kernel event bus + notifiche compositor (v1)

**Data:** 2026-06-11

## Cosa
Bus pub/sub kernel→compositor (`kernel/src/kevent.rs`: ring 64 slot, publish
IRQ-safe zero-alloc, side-table nomi, lettura a cursore con rilevamento gap);
shutdown/reboot differito annullabile in `power.rs` (request/cancel/pending +
task di enforcement embassy); publish da frame-error/spawn-error del compositor
(APP_CRASHED), out-of-fuel wasmi (APP_FUEL_EXHAUSTED), frame allocator con
isteresi (MEM_LOW); compositor: drain del bus + toast (INFO/WARN, max 3, ~5 s,
click-dismiss) + modale CRIT con countdown e Annulla/Esc; ABI: `wm.poweroff`/
`wm.reboot` ora differite e annullabili (docs aggiornate); builtin debug
`kev-test` + host fn `ruos.kev_test`; self-test boot-checks `KEVENT_TEST`.

## Perché
Notifiche kernel→utente affidabili anche se il desktop egui è morto (rendering
kernel-side `decor`); l'enforcement dello shutdown non dipende dalla UI; il
ring a cursori prepara gratis la futura API app-facing `sys.events_poll` (v2).
Spec: docs/superpowers/specs/2026-06-11-kernel-event-bus-design.md.

## File toccati
- kernel/src/kevent.rs (nuovo)
- kernel/Cargo.toml, kernel/src/main.rs
- kernel/src/power.rs
- kernel/src/wasm/wt/wm.rs
- kernel/src/wasm/fiber.rs
- kernel/src/memory/frames.rs
- kernel/src/wasm/host/proc.rs
- kernel/src/boot/phases/devices.rs
- user/shell/src/main.rs
- docs/api/wm.md, docs/api/ruos.md, docs/api/ruos-window.md
- ruos-desktop/crates/ruos-window/src/lib.rs (submodule)
- docs/superpowers/specs/2026-06-11-kernel-event-bus-design.md (stato + kind TEST)
```

- [ ] **Step 2: aggiorna la spec**

In `docs/superpowers/specs/2026-06-11-kernel-event-bus-design.md`:
- riga 4 `**Stato:**` → `implementato (v1) — vedi CHANGELOG/NN-26-06-11-kernel-event-bus.md`;
- nella tabella del catalogo (sezione 2) aggiungi la riga:

```markdown
| `TEST` | 0x0002 | INFO/WARN | `[marker, 0, 0, 0]` + nome | self-test boot-checks + `ruos.kev_test` (debug) |
```

- [ ] **Step 3: riepilogo finale all'utente**

Riportare: esiti test, modifiche al submodule `ruos-desktop` non committate,
e che NESSUN commit è stato fatto (in attesa di richiesta esplicita).

---

## Note di design (per l'esecutore)

- **Perché un solo lock per ring+nomi:** `name_of(seq)` deve vedere il nome
  coerente con lo slot; due lock separati permetterebbero a un publish
  concorrente di sovrascrivere il nome tra le due letture.
- **Perché il task di enforcement controlla `deadline_tick == deadline`:** un
  `cancel()` seguito da una nuova `request_*` crea un secondo task; il primo,
  al risveglio, NON deve spegnere sulla richiesta nuova (che ha la sua
  deadline). `pool_size = 2` copre l'overlap.
- **Perché `kev_cursor` parte da `current_seq()`:** il self-test boot-checks
  pubblica 70 eventi prima che il compositor parta; partire da 0 li
  ripresenterebbe tutti come toast.
- **Perché `decor::draw_text_at`:** `draw_text` centra verticalmente in
  `buf_h` (pensata per strip piccole tipo taskbar); sul backbuf full-screen
  il testo finirebbe a metà schermo.
- **`ruos.poweroff`/`ruos.reboot` (wasmi, console) e `ruos:gui/power`
  INVARIATI:** la spec cambia solo `wm.*`. Il path console resta immediato; il
  gate component-model usa il never-return nei boot-check.
- **proc_exit volontario di un'app finestra** arriva al frame-error path come
  trap → genera un toast APP_CRASHED (causa 0). Accettato in v1 (è il path
  spec'd); raffinabile in v2 distinguendo il trap di exit.
