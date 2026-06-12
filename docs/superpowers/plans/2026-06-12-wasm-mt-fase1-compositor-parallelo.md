# WASM Multithreading — Fase 1: Compositor parallelo + audit rientranza — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Dispatch le `frame()` delle finestre WASM sveglie come job paralleli sul compute pool SMP (un core per finestra), e blindare ogni host fn `wt/*` raggiungibile da `frame()` contro chiamate concorrenti — senza scheduler preemptive, senza ring 3.

**Architecture:** `Compositor::frame_all()` oggi serializza le `frame.call()` di tutte le finestre sul core GUI (BSP). La Fase 1 la spezza in 3 fasi: (A) selezione + arming deadline epoch sul core GUI, che riempie un'arena statica di descrittori `*mut Window`; (B) dispatch dei `frame()` sul compute pool — stesso pattern di `dispatch_bands` (submit → fallback inline → join con work-steal); (C) adozione size post-join sul core GUI. Effetto voluto: le host fn `wt/*` vengono chiamate da più core → audit di rientranza obbligatorio. Tutto dietro la feature `wm-serial-frames` (ripristina il seriale per bisect).

**Tech Stack:** Rust `no_std`, Wasmtime AOT no_std, compute pool SMP (`kernel/src/smp/pool.rs`), LAPIC IPI wake, epoch watchdog. Build/test via WSL (`Ubuntu-22.04`, repo a `/mnt/w/Work/GitHub/ruos`). Test = boot-check markers + script shell stile `tests/comp-smp-test.sh` + `make run-test`.

---

## Premesse verificate (non re-investigare)

- **`frame_all()`** vive in `kernel/src/wasm/wt/wm.rs:1715-1785`; è chiamata da `run()` a `wm.rs:2430`. Itera `for w in self.wins.iter_mut()` (riga 1717), salta i dormienti (`compute_awake`), arma il deadline epoch per-store (riga 1734), chiama `frame.call` (riga 1735), gestisce crash (publish `APP_CRASHED` + `close_requested`, righe 1737-1755), poi adotta la committed size (righe 1760-1783).
- **`Window`** (`wm.rs:1124-1160`): `store: Store<AppState>`, `inst: Instance`, `awake`, `framed_once`, `sized`, `rect`, `id`, `title`. **`AppState`** (`wm.rs:784-792` = `WtState` + `WmState` + `StoreLimits`) contiene SOLO tipi `Send` (Vec/VecDeque/String/Fd/primitivi) — verificato, niente `Rc`. `Store<AppState>` non è `Send` per Wasmtime, ma ogni job ne è l'unico owner durante il volo (deref di `*mut Window`), quindi nessun accesso concorrente allo stesso store.
- **Pattern di dispatch da replicare** = `dispatch_bands` (`wm.rs:516-622`) con arena statica `BAND_ARENA` (`wm.rs:448-450`), job `composite_band_job` (`wm.rs:475-503`), marker `COMPOSITE_CORE_MASK` (`wm.rs:461-466`).
- **Pool API** (`kernel/src/smp/pool.rs`): `submit(JobFn, &'static [u8]) -> Option<usize>` (54-68, manda IPI wake), `take() -> Option<usize>` (78-85), `run_slot(id, cpu)` (88-105), `poll_done(id) -> Option<(u64,u32)>` (109-119), `is_empty()` (72-74). `JobFn = fn(&[u8]) -> u64` (riga 20). `MAX_JOBS = 64`.
- **Feature gate da mirrorare**: `serial-composite` in `kernel/Cargo.toml:70`, usata a `wm.rs:531-534`.
- **Marker boot-check esistente**: `wm.rs:2576-2585` stampa `composite cores=N [...]` a frame 30; il test `tests/comp-smp-test.sh` lo grep-pa e asserisce `>=2`.
- **Host fn raggiungibili da `frame()` di una finestra GUI = SOLO `wt/*`** (`kernel/src/wasm/wt/{wm,sys,term,net,gfx,gui,component}.rs`). Le `kernel/src/wasm/host/*` sono il runtime **wasmi** (tool CLI), NON raggiungibili da una finestra Wasmtime — non vanno auditate per questa fase.
- **`cpus_online()`** in `kernel/src/cpu/mod.rs:265-267`; `cpu_id()` LAPIC-based (VBox-safe).

## File Structure

- **Modify** `kernel/src/wasm/wt/wm.rs`:
  - Aggiungi `FRAME_ARENA`, `FrameArg`, `FRAME_CORE_MASK`, `take_frame_core_mask()`, `frame_one_job()`, `Compositor::run_frame()`, `Compositor::dispatch_frames()` (accanto agli analoghi band, ~riga 466).
  - Riscrivi `Compositor::frame_all()` (1715-1785) in 3 fasi.
  - Aggiungi il marker `frame cores=` in `run()` (~2576).
- **Modify** `kernel/Cargo.toml`: nuova feature `wm-serial-frames`.
- **Create** `tests/frame-smp-test.sh`: boot 4 vCPU + 2 finestre sveglie, asserisce `frame cores= >= 2` (clone di `comp-smp-test.sh`).
- **Create** `docs/superpowers/specs/2026-06-12-hostfn-reentrancy-audit.md`: il deliverable audit (una riga di esito per host fn `wt/*`).
- **Create** app reactor di stress (path da decidere col workflow `new-ruos-app-workflow`; default `apps/` + sorgente in nuovo progetto SDK): martella `sys.proc_stat` + `term` write su 2 finestre.
- **Create** `CHANGELOG/476-26-06-12-mt-fase1-compositor-parallelo.md` (e le entry successive per ogni commit della fase).
- **Modify** `docs/superpowers/roadmap-rust-os.md` + `docs/api/` se l'audit cambia semantica di una host fn.

---

### Task 0: Bring-up gate — Wasmtime guest su un AP (DE-RISK, fare per primo)

**Perché per primo:** oggi `frame.call()` (esecuzione guest Wasmtime) gira SOLO sul BSP. Le band job NON chiamano Wasmtime. Far girare `frame()` su un Application Processor è **nuovo**: il fault handling del guest (page-fault su accesso OOB → trap Wasmtime; epoch-interrupt → `Trap::Interrupt`) deve funzionare sul core dell'AP, non solo sul BSP. Se l'IDT/handler dei trap è BSP-only, il kernel panica/freeza appena un guest gira su un AP. Questo task lo verifica con UN job, prima di cablare tutto.

**Files:**
- Modify (temporaneo, throwaway): `kernel/src/wasm/wt/wm.rs` — `frame_all()` per provare UNA finestra su un AP.

- [ ] **Step 1: Scrivere il probe temporaneo**

In `frame_all()`, subito prima del `for w in self.wins.iter_mut()` esistente, inserisci un probe one-shot che esegue la PRIMA finestra sveglia via un singolo pool job e logga il core su cui è girata. Codice da incollare in testa a `frame_all` (lo rimuoverai nello Step 5):

```rust
// === BRING-UP PROBE (throwaway, rimosso allo Step 5 del Task 0) ===
{
    use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    static PROBE_DONE: AtomicBool = AtomicBool::new(false);
    static PROBE_CORE: AtomicU32 = AtomicU32::new(u32::MAX);
    static mut PROBE_ARG: usize = 0; // *mut Window
    fn probe_job(input: &[u8]) -> u64 {
        let p = unsafe { core::ptr::read_unaligned(input.as_ptr() as *const usize) };
        let w: &mut Window = unsafe { &mut *(p as *mut Window) };
        if let Ok(f) = w.inst.get_typed_func::<(), ()>(&mut w.store, "frame") {
            w.store.set_epoch_deadline(crate::wasm::wt::FRAME_DEADLINE_TICKS);
            let _ = f.call(&mut w.store, ());
        }
        PROBE_CORE.store(crate::cpu::cpu_id(), Ordering::SeqCst);
        0
    }
    if !PROBE_DONE.load(Ordering::SeqCst) && crate::cpu::cpus_online() >= 2 {
        if let Some(w) = self.wins.iter_mut().find(|w| Self::compute_awake(w, self.frame_no)) {
            unsafe { PROBE_ARG = w as *mut Window as usize; }
            let bytes: &'static [u8] = unsafe {
                core::slice::from_raw_parts(
                    core::ptr::addr_of!(PROBE_ARG) as *const u8,
                    core::mem::size_of::<usize>(),
                )
            };
            if let Some(id) = crate::smp::pool::submit(probe_job, bytes) {
                loop {
                    if crate::smp::pool::poll_done(id).is_some() { break; }
                    if let Some(slot) = crate::smp::pool::take() {
                        crate::smp::pool::run_slot(slot, crate::cpu::cpu_id());
                    } else { core::hint::spin_loop(); }
                }
                crate::binfo!("wm", "PROBE frame() ran on core={}",
                    PROBE_CORE.load(Ordering::SeqCst));
                PROBE_DONE.store(true, Ordering::SeqCst);
            }
        }
    }
}
// === FINE PROBE ===
```

- [ ] **Step 2: Build + run headless 4 vCPU**

Run (WSL):
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && make run-test CARGO_FEATURES=boot-checks QEMU_SMP=4'
```
(Se `QEMU_SMP` non è una variabile del Makefile, verifica come il Makefile passa `-smp` a QEMU e impostala di conseguenza; `comp-smp-test.sh` lo fa già — guardalo.)

Expected: nel log compare `wm PROBE frame() ran on core=N`. **GO** se `N` è un core qualsiasi (anche 0) E il boot prosegue fino al marker di successo senza panic/freeze. Ideale: `N != 0` (girata davvero su un AP).

- [ ] **Step 3: Verificare il trap di un guest su AP**

Per provare che un trap del guest su un AP è gestito (non panica il kernel): abbassa temporaneamente `FRAME_DEADLINE_TICKS` nel probe a un valore che fa scattare il watchdog (es. `1`) e ri-builda. Expected: il probe logga il core, il `f.call` ritorna `Err(Trap::Interrupt)` (loggato come errore dentro il match — qui il `let _ =` lo ignora ma NON deve panicare il kernel), e il boot prosegue.

Run: stesso comando dello Step 2 con la modifica al deadline.
Expected: nessun panic; boot completa. **Se il kernel panica/freeza qui → STOP**: il trap routing è BSP-only. Apri un'indagine separata (systematic-debugging) sul fault/epoch handler per-core PRIMA di proseguire — è il prerequisito hard dell'intera fase.

- [ ] **Step 4: Verifica su VirtualBox (regola progetto: cambi CPU-sensitive su VBox)**

Builda l'ISO e bootala su VBox con ≥4 vCPU. Expected: stesso `PROBE frame() ran on core=N`, desktop renderizza, nessun freeze.
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && make iso CARGO_FEATURES=boot-checks'
```
(VBox: monta `ruos.iso`, 4+ vCPU, avvia, osserva schermo/netconsole.)

- [ ] **Step 5: Rimuovere il probe**

Cancella il blocco `=== BRING-UP PROBE ===` da `frame_all()`. NON committare il probe.

- [ ] **Step 6: Annotare l'esito**

Scrivi una riga in cima alla bozza di `docs/superpowers/specs/2026-06-12-hostfn-reentrancy-audit.md`: "Bring-up gate (Task 0): frame() su AP OK / trap su AP gestito — verificato QEMU 4 vCPU + VBox il 2026-06-12." (Niente commit ancora — l'audit doc si committa al Task 6.)

---

### Task 1: Audit di rientranza host fn `wt/*` (il cuore della fase)

**Files:**
- Read-only: `kernel/src/wasm/wt/{wm,sys,term,net,gfx,gui,component}.rs` + i moduli dello stato condiviso che toccano.
- Create: `docs/superpowers/specs/2026-06-12-hostfn-reentrancy-audit.md`.

Questo task è investigazione + scrittura. Nessun cambio di comportamento del kernel; eventuali fix vanno al Task 2. Censisci OGNI `func_wrap` nei file `wt/*` e, per ciascuna, scrivi UNA riga di esito.

- [ ] **Step 1: Censimento `func_wrap`**

Run:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && grep -rn "func_wrap" kernel/src/wasm/wt/'
```
Per ogni risultato annota: modulo, nome fn, stato globale toccato.

- [ ] **Step 2: Verificare le tre regole d'oro per ogni fn**

Per ciascuna host fn, verifica e annota:
1. Non tiene DUE lock kernel annidati senza ordine globale documentato.
2. Non tiene uno spin-lock attraverso un'operazione O(n) non bounded (alloc grosse, copy di buffer interi) contendibile da IRQ o altro core. (Pattern SICURO già in uso: prendi il lock, copia il minimo in una `Vec` locale, **droppa il lock**, POI scrivi nella guest memory — vedi `wm.window_list` a `wm.rs:1018-1037` e `net.resolve_poll`.)
3. Lo stato per-finestra vive in `WmState`/`AppState` (per-store), MAI in static globali indicizzati "dalla finestra corrente".

- [ ] **Step 3: Approfondire le aree ad alto rischio (concorrenza cross-core reale)**

Queste sono ora chiamabili da più core insieme (una finestra per core). Verifica che il primitivo di lock sia cross-core-safe (IrqMutex/spin leaf), non un'assunzione single-core:

- **`term.read/write/resize`** (`wt/term.rs`): le ring PTY (`master_output_try`, `master_input_push`, `set_winsize`) sono condivise con la fiber shell sul BSP. Apri `kernel/src/pty/*`: conferma che le ring usano un lock vero (IrqMutex/spin), non un `static mut` o un'assunzione "solo il BSP tocca la ring". Se un AP e il BSP possono pushare/drainare la stessa ring concorrentemente, il lock DEVE proteggerle. Annota il tipo di lock e l'esito.
- **`net.*`** (`wt/net.rs`): `RESOLVES` (IrqMutex), `dial/read/write` su `NET`. Regola esistente "mai tenere `NET` attraverso una wait". Conferma che da un job AP: (a) `NET` non venga tenuto attraverso `resolve_start`/spawn; (b) `resolve_start` (spawna `dns_task` su core 0 via `executor::spawn_on`) e `resolve_poll` non corrano sullo stesso slot `RESOLVES` senza lock — il slot è scritto sotto `RESOLVES.lock()` sia dal guest sia da `dns_task`? Verifica `dns_task` in `wt/net.rs`. Annota.
- **`gfx.poll_event/pending/blit`** (`wt/gfx.rs`): `fold_mouse`/`pop`/`pending` drenano la coda input. Il run loop sul core GUI chiama già `gfx::fold_mouse()`+`gfx::pop()` (`wm.rs:2267`) — ma quello avviene FUORI da `frame_all` (prima), quindi non concorrente con i frame job. Però una finestra bg/overlay potrebbe chiamare `gfx.poll_event` dentro `frame()`, ora su un AP, mentre un'altra finestra fa lo stesso su un altro AP. Apri `kernel/src/gfx/*`: conferma che la coda eventi e il blit usano un lock cross-core-safe. Annota.
- **`sys.proc_stat/cpustat/meminfo`** (`wt/sys.rs`): registry processi + cpustat. Verifica niente alloc grossa sotto spin-lock contendibile. Annota.
- **`wm.window_list/app_list`**: `WINDOW_SNAPSHOT`/`APP_CATALOG` (IrqMutex) sono SCRITTE dal core GUI nella fase deferred di `run()` (dopo `frame_all`, `wm.rs:2537`), e LETTE dai frame job durante il volo. Scrittura e lettura NON sono concorrenti (fasi diverse del loop), ma due frame job possono leggerle insieme — lettura concorrente sotto IrqMutex è sicura. Conferma e annota.
- **`kevent` BUS** (publish da `run_frame` su crash; `sys.events_poll`): conferma che `kevent::publish_named` e `read_since` usano IrqMutex/try_lock cross-core-safe (publish ora avviene da un AP). Apri `kernel/src/kevent.rs`. Annota.
- **`power`** (`wm.power_pending/cancel/poweroff/reboot`): apri `kernel/src/power.rs`, conferma `PENDING` IrqMutex senza nesting. Annota.

- [ ] **Step 4: Scrivere il doc di audit**

Crea `docs/superpowers/specs/2026-06-12-hostfn-reentrancy-audit.md` con: intestazione (data, scopo, esito bring-up gate dal Task 0), una **tabella con una riga per host fn** (modulo | fn | stato condiviso | lock | esito: SAFE / FIX-REQUIRED + nota), e una sezione finale **"FIXES REQUIRED"** che elenca puntualmente ogni violazione trovata (file:riga + cosa cambiare). Se non emerge nessun fix, scrivilo esplicitamente ("audit pulito, nessun fix richiesto").

- [ ] **Step 5: NON committare ancora** (il doc si committa al Task 6 insieme ai fix; così l'audit e i suoi fix restano nello stesso commit della host fn, come da regola docs/api).

---

### Task 2: Applicare i fix dell'audit

**Files:** quelli elencati nella sezione "FIXES REQUIRED" del doc di audit (Task 1, Step 4).

**Se l'audit è pulito (nessun FIX-REQUIRED): salta al Task 3.**

Per OGNI item della sezione "FIXES REQUIRED", applica il fix puntuale. I fix tipici attesi e la loro forma:

- [ ] **Step 1: Lock mancante/non cross-core su una ring condivisa**

Se una ring (es. PTY o coda gfx) si affida a "solo il BSP la tocca": avvolgila in `crate::sync::IrqMutex` (definito in `kernel/src/sync/mod.rs`) e prendi/rilascia il lock dentro ogni accessor, tenendolo per il minimo (push/pop di un elemento), MAI attraverso una copia di buffer intero o una guest-memory write. Pattern di riferimento: `net.resolve_poll` (lock → copia minima → drop → write).

- [ ] **Step 2: Lock tenuto attraverso operazione non-bounded**

Se una fn tiene un lock attraverso una alloc grossa o una copy O(n): ristruttura in "lock → copia il minimo in `Vec` locale → drop lock → fai il lavoro pesante". Mostra il diff esatto nel doc di audit.

- [ ] **Step 3: Build di verifica**

Run:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && make run-test'
```
Expected: boot completa, stringa di successo presente. (Ancora seriale — il dispatch parallelo arriva al Task 4; qui verifichi solo che i fix non rompono il seriale.)

- [ ] **Step 4: Aggiornare `docs/api/` se cambia una semantica app-facing**

Se un fix cambia la signature o la semantica di una host fn `wm`/`sys`/`term`: aggiorna la pagina corrispondente in `docs/api/` + il "Last reviewed", e l'`extern "C"` in `ruos-desktop/crates/ruos-window/src/lib.rs` (regola CLAUDE.md). Se nessun fix tocca l'ABI app-facing, salta.

---

### Task 3: Feature gate + scaffolding (arena, marker, job)

**Files:**
- Modify: `kernel/Cargo.toml` (feature `wm-serial-frames`).
- Modify: `kernel/src/wasm/wt/wm.rs` (arena + marker + job + `run_frame`).

- [ ] **Step 1: Aggiungere la feature**

In `kernel/Cargo.toml`, sotto `[features]` (accanto a `serial-composite` a riga 70), aggiungi:
```toml
# Force window frame() execution to run serially on the GUI core (no SMP
# dispatch). Bisect baseline for the parallel compositor; mirror of
# serial-composite for the frame() loop. Default = parallel.
wm-serial-frames = []
```

- [ ] **Step 2: Aggiungere arena, marker e job in `wm.rs`**

In `kernel/src/wasm/wt/wm.rs`, subito dopo il blocco `COMPOSITE_CORE_MASK` / `take_composite_core_mask` (riga 466), inserisci:
```rust
/// Max window frame() jobs dispatched in parallel per frame. One per pool slot;
/// capped at pool::MAX_JOBS (64). Mirror of MAX_BANDS for the frame loop.
const MAX_FRAME_JOBS: usize = 64;

/// One window's frame() job descriptor. Carries a raw `*mut Window` (as usize)
/// into the live `wins` Vec. The GUI core fills `[0, n)` with DISTINCT windows,
/// submits, and BLOCKS on the join before touching `wins` again — so no two
/// in-flight jobs alias the same Window, and the GUI core never reads/mutates
/// `wins` during the flight (same invariant as BAND_ARENA).
#[repr(C)]
#[derive(Copy, Clone)]
struct FrameArg {
    win: usize, // *mut Window into self.wins
}

static mut FRAME_ARENA: [FrameArg; MAX_FRAME_JOBS] =
    [FrameArg { win: 0 }; MAX_FRAME_JOBS];

/// Distinct cores that ran a frame() job in the most recent frame (bitset by
/// cpu_id). Reset at the top of frame_all, read by the boot-check marker to
/// prove parallel frame() execution.
static FRAME_CORE_MASK: AtomicU32 = AtomicU32::new(0);

/// Read + clear the frame core mask (boot-check marker support).
pub fn take_frame_core_mask() -> u32 {
    FRAME_CORE_MASK.swap(0, Ordering::SeqCst)
}

/// Pool job: run ONE window's frame(). `input` is a byte view of one FrameArg
/// in FRAME_ARENA. Returns 0 (unused).
///
/// SAFETY: the dispatcher guarantees (a) `input` is exactly size_of::<FrameArg>()
/// bytes of a valid FrameArg, (b) `win` points at a live Window uniquely owned
/// by THIS job for its lifetime (GUI core blocks on join before reusing it),
/// (c) the epoch deadline is already armed on that Window's store, (d) no other
/// core touches this Window concurrently.
fn frame_one_job(input: &[u8]) -> u64 {
    if input.len() < core::mem::size_of::<FrameArg>() {
        return 0;
    }
    let arg: FrameArg = unsafe { core::ptr::read_unaligned(input.as_ptr() as *const FrameArg) };
    // SAFETY: unique live Window for the flight (see fn contract).
    let w: &mut Window = unsafe { &mut *(arg.win as *mut Window) };
    Compositor::run_frame(w);
    let cpu = crate::cpu::cpu_id();
    if cpu < 32 {
        FRAME_CORE_MASK.fetch_or(1u32 << cpu, Ordering::SeqCst);
    }
    0
}
```

- [ ] **Step 3: Estrarre `run_frame` come fn associata senza `&self`**

In `impl Compositor`, accanto a `frame_all` (riga 1715), aggiungi la fn che esegue una singola `frame()` (corpo estratto dalle righe 1724-1756, SENZA l'arming del deadline e SENZA l'adozione size — quelli restano sul core GUI):
```rust
/// Execute ONE window's frame(): get the typed func, call it, handle a crash.
/// No `&self`: callable from any core as a pool job OR inline on the GUI core.
/// PRECONDITION: the caller has already armed `w.store`'s epoch deadline.
/// The committed-size adoption + `framed_once` are done by the caller after the
/// join (they read/mutate the store on the GUI core, kept serial).
fn run_frame(w: &mut Window) {
    let frame = match w.inst.get_typed_func::<(), ()>(&mut w.store, "frame") {
        Ok(f) => f,
        Err(_) => return,
    };
    match frame.call(&mut w.store, ()) {
        Ok(()) => {}
        Err(e) => {
            // Anche un proc_exit volontario arriva qui come trap — il marker
            // WATCHDOG distingue il kill da deadline dal trap/panic.
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
    }
}
```

- [ ] **Step 4: Build (deve compilare, comportamento ancora invariato)**

`run_frame`/`frame_one_job`/arena sono per ora non chiamati (dead code dietro warning, accettabile in questo step intermedio). Verifica solo che compili:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && make iso'
```
Expected: build OK (eventuali warning `dead_code` su `run_frame`/`frame_one_job`/arena sono attesi finché il Task 4 non li collega; risolti lì).

---

### Task 4: Riscrivere `frame_all` in 3 fasi + `dispatch_frames`

**Files:**
- Modify: `kernel/src/wasm/wt/wm.rs` (`frame_all` 1715-1785 + nuova `dispatch_frames`).

- [ ] **Step 1: Scrivere il test PRIMA (script di boot-check)**

Crea `tests/frame-smp-test.sh` clonando `tests/comp-smp-test.sh` ma:
- builda l'ISO con `CARGO_FEATURES=boot-checks` (parallelo di default — NON `serial-composite`), `-smp 4`;
- l'init script deve spawnare ALMENO 2 finestre che restano sveglie (es. due app che chiamano `wm.stay_awake()` ogni frame, o l'orologio + un'app animata — vedi Task 5 per la reactor; in prima battuta puoi usare due istanze della demo egui se restano sveglie);
- grep del marker `frame cores=` (analogo a `composite cores=`), estrae `K`, asserisce `K >= 2`.

Copia la struttura esatta da `comp-smp-test.sh` (build seriale-vs-parallelo, grep `sed -nE 's/.*frame cores=([0-9]+).*/\1/p'`, assert `[ "$NCORES" -ge 2 ]`).

- [ ] **Step 2: Eseguire il test → deve FALLIRE**

Run:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && bash tests/frame-smp-test.sh'
```
Expected: FAIL — il marker `frame cores=` non esiste ancora (o `K < 2`), perché `frame_all` è ancora seriale.

- [ ] **Step 3: Aggiungere `dispatch_frames`**

In `impl Compositor`, accanto a `run_frame`, aggiungi:
```rust
/// Run the first `n` FRAME_ARENA jobs. With `wm-serial-frames` (or ≤1 CPU)
/// runs them inline on the GUI core; otherwise dispatches one pool job per
/// window across the compute pool and JOINS before returning. Mirror of
/// dispatch_bands. PRECONDITION: FRAME_ARENA[0..n] filled with DISTINCT live
/// Windows whose deadlines are already armed; the GUI core must not touch
/// `wins` until this returns.
fn dispatch_frames(n: usize) {
    if n == 0 { return; }

    #[cfg(feature = "wm-serial-frames")]
    {
        for k in 0..n {
            // SAFETY: distinct live Window per slot (filled by frame_all).
            let w = unsafe { &mut *((FRAME_ARENA[k].win) as *mut Window) };
            Self::run_frame(w);
            let cpu = crate::cpu::cpu_id();
            if cpu < 32 { FRAME_CORE_MASK.fetch_or(1u32 << cpu, Ordering::SeqCst); }
        }
    }

    #[cfg(not(feature = "wm-serial-frames"))]
    {
        let mut ids: [usize; MAX_FRAME_JOBS] = [usize::MAX; MAX_FRAME_JOBS];
        let mut n_submitted = 0usize;
        for k in 0..n {
            // A `&'static [u8]` view of arena slot k. FRAME_ARENA is a real
            // `static`, so the slice is genuinely 'static; the GUI core blocks
            // on the join below before reusing slot k next frame.
            let bytes: &'static [u8] = unsafe {
                core::slice::from_raw_parts(
                    core::ptr::addr_of!(FRAME_ARENA[k]) as *const u8,
                    core::mem::size_of::<FrameArg>(),
                )
            };
            match crate::smp::pool::submit(frame_one_job, bytes) {
                Some(id) => { ids[k] = id; n_submitted += 1; }
                None => { break; } // pool full: leftovers run inline below
            }
        }

        // 1-CPU fallback: drain queued jobs inline so we never wait on cores
        // that aren't there.
        if crate::cpu::cpus_online() <= 1 {
            while let Some(slot) = crate::smp::pool::take() {
                crate::smp::pool::run_slot(slot, crate::cpu::cpu_id());
            }
        }

        // Pool-full leftover: run the unsubmitted frames inline on the GUI core.
        for k in n_submitted..n {
            // SAFETY: distinct live Window per slot; not submitted to any AP,
            // so running it here cannot race an in-flight job.
            let w = unsafe { &mut *((FRAME_ARENA[k].win) as *mut Window) };
            Self::run_frame(w);
            let cpu = crate::cpu::cpu_id();
            if cpu < 32 { FRAME_CORE_MASK.fetch_or(1u32 << cpu, Ordering::SeqCst); }
        }

        // Join: block until every submitted frame job is DONE. Work-steal so the
        // GUI core makes forward progress if APs are slow (same rationale as
        // dispatch_bands; stealing a frame job just runs Wasmtime on the GUI
        // core, which is where it ran before).
        for k in 0..n {
            if ids[k] == usize::MAX { continue; }
            loop {
                if crate::smp::pool::poll_done(ids[k]).is_some() { break; }
                if let Some(slot) = crate::smp::pool::take() {
                    crate::smp::pool::run_slot(slot, crate::cpu::cpu_id());
                } else {
                    core::hint::spin_loop();
                }
            }
        }
    }
}
```

- [ ] **Step 4: Riscrivere `frame_all` in 3 fasi**

Sostituisci INTERAMENTE il corpo di `frame_all` (1715-1785) con:
```rust
fn frame_all(&mut self) {
    let fno = self.frame_no;
    FRAME_CORE_MASK.store(0, Ordering::SeqCst); // per-frame: il marker legge questo frame

    // --- Fase A (core GUI): selezione + arming deadline → riempi l'arena. ---
    let mut n = 0usize;
    for w in self.wins.iter_mut() {
        if !Self::compute_awake(w, fno) {
            w.awake = false;
            continue; // dormiente: niente frame(); la surface in cache resta valida
        }
        w.awake = true;
        w.last_active_frame = fno;
        // Watchdog: riarma il deadline epoch PRIMA dell'entry nel guest (è
        // relativo all'epoch corrente). Primo frame più largo. Fatto sul core
        // GUI perché serve `self.frame_deadline_override`.
        let ticks = self.frame_deadline_override.unwrap_or(if w.framed_once {
            crate::wasm::wt::FRAME_DEADLINE_TICKS
        } else {
            crate::wasm::wt::FIRST_FRAME_DEADLINE_TICKS
        });
        w.store.set_epoch_deadline(ticks);
        if n < MAX_FRAME_JOBS {
            // SAFETY: ogni slot riceve un elemento DISTINTO di `wins`; il core
            // GUI non tocca `wins` finché dispatch_frames non ha joinato.
            unsafe { FRAME_ARENA[n] = FrameArg { win: w as *mut Window as usize }; }
            n += 1;
        } else {
            // Overflow (>64 finestre sveglie): esegui inline subito.
            Self::run_frame(w);
            let cpu = crate::cpu::cpu_id();
            if cpu < 32 { FRAME_CORE_MASK.fetch_or(1u32 << cpu, Ordering::SeqCst); }
        }
    }

    // --- Fase B: esegui le frame() in parallelo sul compute pool (o inline). ---
    Self::dispatch_frames(n);

    // --- Fase C (core GUI, dopo il join): adotta la committed size. ---
    for w in self.wins.iter_mut() {
        if !w.awake { continue; }
        // Considera la finestra "avviata" solo quando ha prodotto una surface.
        if w.store.data().win.committed {
            w.framed_once = true;
        }
        // CSD: il hit-rect deve tracciare il committed win_w×win_h. Tieni l'origine
        // (x,y); adotta solo w/h alla prima commit (configure bootstrap).
        let (cw, ch) = { let s = w.store.data(); (s.win.win_w, s.win.win_h) };
        if cw != 0 && ch != 0 && !w.sized {
            let (rx, ry, _, _) = w.rect;
            w.rect = (rx, ry, cw, ch);
            let s = w.store.data_mut();
            s.win.target_w = cw;
            s.win.target_h = ch;
            w.sized = true;
        }
    }
}
```

- [ ] **Step 5: Aggiungere il marker `frame cores=` in `run()`**

In `run()` accanto al marker composite (`wm.rs:2576-2585`), aggiungi un secondo flag/blocco one-shot a frame 30:
```rust
if !frame_marker_done && self.frame_no >= 30 {
    let mask = take_frame_core_mask();
    let mut cores: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
    for c in 0..32u32 {
        if mask & (1u32 << c) != 0 { cores.push(c); }
    }
    crate::binfo!("wm", "frame cores={} {:?}", cores.len(), cores);
    frame_marker_done = true;
}
```
Dichiara `let mut frame_marker_done = false;` accanto a `marker_done` (cerca `marker_done` nel `run()` e aggiungi il gemello). Gate il blocco dietro `#[cfg(feature = "boot-checks")]` se il marker composite lo è (controlla come è gated quello esistente e fai uguale).

- [ ] **Step 6: Eseguire il test → deve PASSARE**

Run:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && bash tests/frame-smp-test.sh'
```
Expected: PASS — `frame cores=K` con `K >= 2`.

- [ ] **Step 7: Verificare il fallback seriale (regressione)**

Run con la feature:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && make run-test CARGO_FEATURES=wm-serial-frames'
```
Expected: boot completa, stringa di successo. Comportamento identico a oggi (`frame cores=1`).

- [ ] **Step 8: Commit**

```bash
git add kernel/src/wasm/wt/wm.rs kernel/Cargo.toml tests/frame-smp-test.sh
git commit -m "feat(wm): parallel window frame() dispatch on compute pool + frame-cores marker"
```
(Solo se sei su un branch non-default; altrimenti crea prima `git switch -c feat/wm-parallel-frames`. NON pushare senza richiesta esplicita.)

---

### Task 5: App reactor di stress host-fn

**Files:**
- Create: sorgente app reactor (path da concordare — usa il workflow `new-ruos-app-workflow`: CHIEDI dove scaffoldare, NON è fisso) + `.cwasm` copiato in `ruos/apps/`.

**Prima di iniziare: invoca il workflow `new-ruos-app-workflow` (memoria) e CHIEDI all'utente dove creare il progetto SDK.** Lo scaffolding via `demo-apps-sdk`, build, e copia del `.cwasm` seguono quel workflow.

- [ ] **Step 1: Scrivere la reactor**

App egui minimale che ogni `frame()`:
- chiama `sys.proc_stat` (lettura registry processi) in loop M volte;
- apre/usa un PTY e fa `term.write` di qualche byte;
- chiama `wm.stay_awake()` per restare sveglia ogni frame (così resta nel set parallelo).
Scopo: martellare le host fn ad alto rischio da DUE finestre concorrenti (due istanze su due core).

- [ ] **Step 2: Build + copia `.cwasm`**

Segui `new-ruos-app-workflow` (build SDK → copia `*.cwasm` in `ruos/apps/`), poi `make iso` per includerla in `/bin`.

- [ ] **Step 3: Boot con 2 reactor + 4 vCPU**

Init script che spawna 2 istanze della reactor. Run con `CARGO_FEATURES=boot-checks`, `-smp 4`. Expected: nessun deadlock/panic, `frame cores >= 2`, il marker di stress (vedi Step 4) verde.

- [ ] **Step 4: Marker anti-deadlock**

Fai loggare alla reactor (via `gfx.gfx_debug`/`binfo` host) un contatore di frame; il test asserisce che dopo N secondi il contatore di ENTRAMBE le finestre è avanzato di ≥ M (nessuna finestra bloccata su un lock). Timeout dello script = FAIL.

- [ ] **Step 5: Commit** (app + test marker).

---

### Task 6: Committare il doc di audit + fix + roadmap

**Files:**
- `docs/superpowers/specs/2026-06-12-hostfn-reentrancy-audit.md` (Task 1).
- Eventuali fix del Task 2.
- `docs/superpowers/roadmap-rust-os.md`, `docs/api/*` se toccati.

- [ ] **Step 1: Aggiornare la roadmap**

In `docs/superpowers/roadmap-rust-os.md` (e nella tabella di `docs/superpowers/specs/2026-06-12-wasm-multithreading-roadmap-design.md`), marca Fase 1 come implementata con riferimento a questo piano + al doc di audit.

- [ ] **Step 2: Changelog**

Crea `CHANGELOG/476-26-06-12-mt-fase1-compositor-parallelo.md` (formato CLAUDE.md: # 476 — titolo, Data, Cosa, Perché, File toccati). Se hai già speso numeri 476+ in commit precedenti della fase, usa il successivo libero (ricontrolla `ls CHANGELOG | sed -E 's/^([0-9]+)-.*/\1/' | sort -n | tail -1`).

- [ ] **Step 3: Commit**

```bash
git add docs/superpowers/specs/2026-06-12-hostfn-reentrancy-audit.md docs/superpowers/roadmap-rust-os.md CHANGELOG/476-26-06-12-mt-fase1-compositor-parallelo.md
git commit -m "docs(mt): host-fn reentrancy audit + Fase 1 roadmap update"
```

---

### Task 7: Verifica finale go/no-go Fase 1 → Fase 2

Criteri dalla spec (riga 38-40): run-test + comp-smp verdi col compositor parallelo; checklist audit 100%; stress host-fn verde su VBox.

- [ ] **Step 1: run-test verde (parallelo, default)**
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && make run-test'
```
Expected: PASS.

- [ ] **Step 2: comp-smp verde (il band compositing condivide il pool — niente starvation coi frame job)**
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && bash tests/comp-smp-test.sh'
```
Expected: PASS (`composite cores >= 2`). I frame job sono joinati PRIMA del present, quindi non si sovrappongono alle band per costruzione.

- [ ] **Step 3: frame-smp verde**
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && bash tests/frame-smp-test.sh'
```
Expected: PASS.

- [ ] **Step 4: Stress reactor verde su VBox (OBBLIGATORIO — cambio CPU-sensitive)**

Builda l'ISO, boota su VBox con ≥4 vCPU + init che spawna 2 reactor. Osserva (schermo o netconsole `CARGO_FEATURES=netconsole`): desktop fluido, entrambe le reactor avanzano, nessun freeze/deadlock per ≥60s.

- [ ] **Step 5: Aggiornare il design spec con l'esito go/no-go**

In `docs/superpowers/specs/2026-06-12-wasm-multithreading-roadmap-design.md`, annota nella tabella roadmap che la Fase 1 è ✅ e i criteri go/no-go sono soddisfatti (con date). Apri la Fase 2 alla ri-specifica.

---

## Self-Review (eseguito)

**Spec coverage:**
- §1 Architettura dispatch (3 fasi, selezione serial / frame parallelo / post-join serial) → Task 4. ✓
- Feature gate `wm-serial-frames` → Task 3 Step 1 + Task 4 Step 7. ✓
- Vincoli correttezza (AppState Send, deadline per-store, W^X codice AOT già mappato) → premesse verificate + Task 0 (bring-up wasmtime su AP). ✓
- §2 Audit rientranza host fn + doc dedicato → Task 1 + Task 2. ✓ (corretto lo scope: solo `wt/*`, NON `host/*` = wasmi.)
- §3 Telemetria/test (marker `frame cores=`, stress reactor, comp-smp, VBox, serial regression) → Task 4 (marker+test), Task 5 (reactor), Task 7 (go/no-go). ✓
- §4 Parametri (max job = min(sveglie, core); stesso pool; fallback inline; feature gate) → `dispatch_frames` (Task 4 Step 3). ✓

**Rischio non esplicito nella spec ma reale:** prima esecuzione Wasmtime su un AP (trap/fault per-core). Coperto da Task 0 come gate hard. ✓

**Type consistency:** `FrameArg.win: usize`, `frame_one_job`, `Compositor::run_frame`, `Compositor::dispatch_frames`, `FRAME_CORE_MASK`/`take_frame_core_mask`, `FRAME_ARENA`, `MAX_FRAME_JOBS` — nomi coerenti tra Task 3 e Task 4. `run_frame` NON arma il deadline e NON adotta la size (lo fa `frame_all`) — coerente in entrambe le definizioni. ✓

**Placeholder scan:** i fix del Task 2 sono condizionati all'output del Task 1 (audit) — inevitabile per un audit; mitigato dando la FORMA esatta dei fix attesi (pattern lock→copia→drop→write) e un percorso esplicito "se audit pulito, salta". ✓
