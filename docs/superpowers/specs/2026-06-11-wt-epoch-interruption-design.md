# Epoch interruption Wasmtime — watchdog sui `frame()` del compositor — design

**Data:** 2026-06-11
**Stato:** proposta (item D dell'analisi prestazioni viewer/Blitz)
**Area:** `kernel/src/wasm/wt/mod.rs`, `kernel/src/wasm/wt/wm.rs`,
`kernel/src/timer.rs`, `tools/wt-precompile/`

## Problema

Le app finestra del desktop sono `.cwasm` Wasmtime AOT eseguite dal compositor
kernel-side. Il compositor chiama l'export `frame()` di ogni finestra in modo
**sincrono** (`frame_all`, `kernel/src/wasm/wt/wm.rs:1553-1601`, `frame.call` a
`wm.rs:1563`) dentro il run loop che possiede la CPU del GUI core
(`Compositor::run`, `wm.rs:1747`). Non c'è preemption: l'engine
(`engine_config`, `kernel/src/wasm/wt/mod.rs:282-308`) non configura né
`epoch_interruption`, né fuel, né async.

Conseguenza: un `frame()` pesante o impazzito blocca **tutto** il desktop —
niente input routing, niente present, niente altre finestre — finché non
ritorna. Numeri misurati col GATE Blitz (relayout Stylo di una pagina HTML):
6400 nodi = **28 ms** di resolve; estrapolando a 50k nodi ≈ **220 ms** per
frame. Un loop infinito nel guest = desktop congelato per sempre (resta vivo
solo l'executor sul BSP: SSH, net).

## Fatti verificati (wasmtime 45.0.0, runtime-only no_std)

1. **`epoch_interruption` è un Tunable hashato nel `.cwasm`**
   (`wasmtime-environ-45.0.0/src/tunables.rs:96`, default `false`). Cambiarlo
   nel Config = **ogni `.cwasm` esistente viene rifiutato al deserialize** con
   l'errore exact-match dei tunables — identico all'incidente
   `memory_reservation` (changelog 422 → 450/451). La regola "config del
   kernel ≡ config di `tools/wt-precompile` byte-per-byte" si applica in pieno.
2. **`Engine::increment_epoch()` è IRQ-safe**: un singolo
   `fetch_add(1, Relaxed)` su un atomico dell'engine
   (`wasmtime-45.0.0/src/engine.rs:853-855`), documentato signal-safe, gated
   solo `#[cfg(target_has_atomic = "64")]` → ok da handler di interrupt x86-64.
3. **Senza la feature `async` non esiste resume.** Alla scadenza del deadline
   il libcall `new_epoch` (emesso dal codice AOT a ogni entry di funzione +
   backedge di loop — funziona anche con `signals_based_traps(false)`, è un
   check esplicito, non un segnale) consulta il comportamento configurato sullo
   Store:
   - default / `epoch_deadline_trap()` → **trap** `Trap::Interrupt`
     (`src/runtime/vm/libcalls.rs:1257-1271`): la chiamata `frame.call` ritorna
     `Err`, lo stack guest è svolto, **nessuna ripresa possibile**;
   - `epoch_deadline_callback(...)` → il callback può ritornare
     `UpdateDeadline::Interrupt` (trap) o `Continue(n)` (estende il deadline e
     prosegue). Le varianti `Yield`/`YieldCustom` sono
     `#[cfg(feature = "async")]` (`src/runtime/store.rs:397-422`) → **non
     disponibili** nel nostro build (`runtime, custom-virtual-memory,
     custom-sync-primitives, component-model` — `kernel/Cargo.toml:23`).
4. **Deadline = stato per-Store, relativo all'epoch corrente**:
   `Store::set_epoch_deadline(n)` va riarmato prima di OGNI entry nel guest.
   **Attenzione**: con `epoch_interruption` attivo, uno Store che NON imposta
   un deadline ha deadline 0 → **trappa immediatamente** al primo check. Ogni
   sito che crea uno Store wasmtime va aggiornato (censimento sotto).
5. L'epoch non interrompe le **host call bloccanti** (es. `vfs::block_on`
   dentro una host fn): il watchdog copre solo codice guest in esecuzione.

## Scelta di design: watchdog (trap), non resume cooperativo

Il **resume cooperativo** (deadline → yield al loop del compositor → riprendi
il frame al giro dopo) richiederebbe `UpdateDeadline::Yield`, cioè la feature
`async` di wasmtime + entrypoint `call_async` + un executor che possa sospendere
lo stack guest su una fiber. Il nostro build è sync-only e il compositor loop
non è un task async (possiede il GUI core). Portare async/fiber dentro il
compositor è un progetto a sé (stack switching no_std, interazione con il
modello SMP shared-nothing). **Respinto** per questa iterazione.

Resta il modello **watchdog**: deadline superato → `Trap::Interrupt` → la
`frame.call` ritorna `Err` → la finestra viene chiusa. Non esiste "prima
infrazione = log e riprova": dopo il trap lo stato interno del guest è
arbitrario (egui a metà pass, allocatore std a metà alloc, lock guest tenuti) —
un successivo `frame()` sulla stessa istanza è undefined behaviour applicativo.
Wasmtime permetterebbe il re-entry, ma sarebbe un falso servizio: la policy è
**trap = finestra morta**, identica al trattamento odierno di qualunque `Err`
da `frame()` (`wm.rs:1565-1570` → `close_requested` → reap). Il valore aggiunto
dell'epoch è trasformare "desktop congelato per sempre" in "finestra colpevole
chiusa entro ~50 ms, desktop vivo".

## Design

### 1. Config engine + precompiler (byte-match)

- `kernel/src/wasm/wt/mod.rs::engine_config`: `config.epoch_interruption(true);`
  accanto agli altri tunables hashati, con commento che rimanda alla regola di
  compatibilità.
- `tools/wt-precompile/src/main.rs`: identica riga, stessa posizione logica.
- `docs/api/README.md` sezione "`.cwasm` compatibility": aggiungere
  `epoch_interruption` all'elenco dei tunables e citare questo cambio come
  secondo esempio storico (dopo il 422).

### 2. Sorgente dell'epoch: timer IRQ BSP a 100 Hz

Il LAPIC timer gira a 100 Hz su ogni core; solo il BSP incrementa il wall clock
(`timer_handler`, `kernel/src/timer.rs:27-49`, invariante single-writer n. 8).
Stesso pattern per l'epoch: **solo il ramo BSP** chiama un nuovo
`crate::wasm::wt::epoch_tick()`:

```rust
/// wt/mod.rs — IRQ-safe: non inizializza MAI l'engine (Once::get, non call_once).
pub fn epoch_tick() {
    if let Some(e) = ENGINE.get() { e.increment_epoch(); }
}
```

Nota implementativa: la `static ENGINE: spin::Once<Engine>` oggi è locale a
`fn engine()` (`wt/mod.rs:200-203`) → va promossa a static di modulo così che
`epoch_tick` possa fare `get()` senza rischiare una `Engine::new` in contesto
IRQ. Costo nel timer handler: una load + un `fetch_add` Relaxed.

**Granularità**: 1 epoch = 10 ms. Il deadline è asincrono rispetto all'inizio
della chiamata: `set_epoch_deadline(N)` scatta dopo un tempo reale in
`[(N−1)·10 ms, N·10 ms]`.

Il guest gira sul GUI core ma legge l'atomico incrementato dal BSP: il check
emesso da cranelift è una load condivisa — cross-core funziona per costruzione,
nessuna dipendenza dai timer degli AP.

### 3. Budget per entry point

Costanti in `wm.rs` (vicino a `GRACE_FRAMES`), in tick di epoch:

| Entry | Costante | Valore | Tempo reale | Razionale |
|---|---|---|---|---|
| `frame()` regime (`framed_once`) | `FRAME_DEADLINE_TICKS` | 6 | 50–60 ms | a 50 ms/frame il desktop è già a ≤20 fps percepiti; oltre = patologia. Margine sopra i 28 ms del GATE 6400 nodi |
| `frame()` primo avvio (`!framed_once`) | `FIRST_FRAME_DEADLINE_TICKS` | 50 | 490–500 ms | primo render Blitz/egui misurato 30–60 ms; font atlas, parse HTML, ecc. — largo per evitare falsi positivi su HW lento |
| `_initialize` (`run_initialize`, `wm.rs:967`) | `INIT_DEADLINE_TICKS` | 100 | ~1 s | init std/allocatore una tantum; un hang qui oggi congela lo spawn |
| probe `manifest()` (`extract_manifest`, `wm.rs:265-266`) | `PROBE_DEADLINE_TICKS` | 10 | ~100 ms | il record è const-data nel data segment, deve essere quasi istantaneo; protegge la scansione ~1 Hz del catalogo |
| `_start` CLI (`run_cwasm`, `wt/mod.rs:255-257`), `run_hello`, bring-up component (`component.rs:37`), spike (`run_reactor_spike`) | `NO_DEADLINE_TICKS` | `u64::MAX / 2` | ∞ | i tool CLI (e `gui.cwasm` legacy) possono legittimamente girare a lungo; il watchdog è SOLO per il compositor. Necessario comunque (fatto verificato n. 4: senza set, trap immediato) |

Riarmo: `w.store.set_epoch_deadline(ticks)` immediatamente **prima** di ogni
`frame.call` in `frame_all` (e prima di `init.call` / `f.call(manifest)`).
Per gli Store one-shot (`_start` ecc.) basta un set alla creazione.

**Configurabilità**: costanti compile-time in questa fase. Un override runtime
(unitctl / cmdline) si aggiunge solo se i numeri si rivelano sbagliati sul
campo; il valore va comunque tenuto ≥ del costo del frame più pesante legittimo
(GATE §test).

### 4. `frame_all`: distinguere il trap epoch e policy

```rust
Err(e) => {
    let epoch = e.downcast_ref::<wasmtime::Trap>()
        == Some(&wasmtime::Trap::Interrupt);
    if epoch {
        crate::bwarn!("wm", "frame() WATCHDOG (epoch deadline) win_id={} '{}': killed", w.id, w.title);
    } else {
        crate::bwarn!("wm", "frame() err win_id={}: {:?}", w.id, e);
    }
    w.store.data_mut().win.close_requested = true;
}
```

- Stessa sorte del trap generico: `close_requested` → reap al giro successivo
  (Store droppato, pid liberato, PTY legato chiuso — `wm.rs:1489-1510`). Il
  desktop resta responsivo: il run loop riprende subito dopo la `Err`.
- Il marker `WATCHDOG` è il discriminante greppabile per test e diagnosi
  (netconsole/seriale), e in fase 3 alimenta la notifica UI.
- Niente contatore "K infrazioni consecutive": non può esistere una seconda
  infrazione, il primo trap uccide (vedi scelta di design). Un'app che viene
  rispawnata dal launcher e trappa di nuovo è visibile dal log ripetuto.
- `_initialize` che trappa per epoch: oggi logga e prosegue (`wm.rs:969-971`)
  lasciando una finestra zombie; con il watchdog il trap epoch in
  `run_initialize` deve marcare la finestra `close_requested` (cioè
  `run_initialize` ritorna `bool` e `spawn_named` annulla lo spawn).

### 5. Censimento Store (obbligatorio, fatto n. 4)

Tutti i siti `Store::new` wasmtime devono impostare un deadline:

| Sito | File | Deadline |
|---|---|---|
| finestre compositor | `wm.rs:1397` (`spawn_named`) | riarmo per-call (§3) |
| probe manifest | `wm.rs:245` (`extract_manifest`) | `PROBE_DEADLINE_TICKS` |
| spike reactor | `wm.rs:985` (`run_reactor_spike`) | `NO_DEADLINE_TICKS` |
| CLI `.cwasm` | `wt/mod.rs:231` (`run_cwasm`) | `NO_DEADLINE_TICKS` |
| hello boot-check | `wt/mod.rs:326` (`run_hello`) | `NO_DEADLINE_TICKS` |
| component bring-up | `component.rs:37` | `NO_DEADLINE_TICKS` |

(`fiber.rs:175` è wasmi, fuori scope — wasmi ha già il fuel metering, step 10.)

## Migrazione `.cwasm` (OBBLIGATORIA, regola changelog 450/451)

Flippare il tunable invalida **ogni** `.cwasm` esistente. Nello stesso commit
della fase 1/2:

1. **Blob embedded nel kernel** (`include_bytes!` in `wm.rs:46-72`):
   `reactor.cwasm`, `reactor_close.cwasm`, `probe.cwasm`, `egui_demo.cwasm`,
   `viewer.cwasm`, `viewer-gate.cwasm` (+ `compositor.cwasm`/`shell.cwasm` e
   ogni altro artefatto AOT in build) → ri-precompilare con il
   `wt-precompile` aggiornato.
2. **`/bin` su ISO**: rigenerato da `make iso` (nessuna azione, ma serve un
   `make iso` completo, non incrementale sui soli `.cwasm`).
3. **Drop folder `apps/`** del repo (es. `apps/viewer.cwasm`): ri-AOT manuale —
   è esattamente il file che nel changelog 450 è sparito in silenzio.
4. **`/mnt/apps` su disco**: una ISO nuova NON tocca il disco — le copie
   stantie vanno sostituite a mano (ora almeno il WARN in seriale lo dice).
5. **SDK demo-apps**: nessun cambio di codice (build.ps1 ricompila
   `wt-precompile` dal checkout a ogni run), ma le app già distribuite vanno
   ri-buildate.
6. Aggiornare `docs/api/README.md` (§"`.cwasm` compatibility") e
   `apps/README.md` citando l'evento.

## Rischi

| Rischio | Mitigazione |
|---|---|
| Falso positivo su primi frame legittimamente pesanti (Blitz 30–60 ms, HW lento) | deadline primo-frame separato (50 tick ≈ 0,5 s); a regime 6 tick > 2× il GATE 6400 nodi |
| Overhead del check epoch nel codice AOT (entry + backedge; atteso ~1–3 %) | misurare col GATE Blitz (avg_resolve_ms) prima/dopo; soglia accettazione ≤5 %. Se sfora, si rinegozia (l'alternativa fuel costerebbe molto di più) |
| Store senza deadline → trap immediato (regressione su tool CLI/boot-check) | censimento §5 + boot-check esistenti (`run_hello`, spike, component) che girano in `make run-test CARGO_FEATURES=boot-checks` |
| `.cwasm` esterni rotti in silenzio | WARN già presente (changelog 450) + docs (451) + migrazione §sopra |
| Trap epoch dentro una host call bloccante: NON scatta (fatto n. 5) | fuori scope: il watchdog copre il codice guest. Le host fn `wm`/`sys` del path frame sono non-bloccanti; documentato come limite |
| Bande SMP di compositing | nessuna interazione: le bande sono codice kernel nativo sugli AP, non wasm; l'unico wasm istrumentato extra-GUI-core sono i tool su ComputeApp, che hanno deadline ∞ |
| `increment_epoch` prima che l'engine esista | `ENGINE.get()` (mai `call_once`) in `epoch_tick` |
| Burst di trap se il BSP resta a lungo con IF=0 (epoch "in ritardo") | il deadline può solo scattare PIÙ TARDI, mai prima: il ritardo è conservativo |

## Piano di test

1. **App spinner deliberata**: nuovo reactor di test (`tools/wt-spin-reactor`,
   gemello di `wt-reactor-close`) il cui `frame()` entra in `loop {}` dopo il
   primo commit. Boot-check (feature `boot-checks`): spawn dello spinner + di
   `react-A`; asserzioni greppabili:
   - compare `frame() WATCHDOG (epoch deadline) win_id=…` entro ~1 s;
   - lo spinner viene reaped (`wins` cala) e `react-A` continua a committare
     (tick avanza) → il desktop NON si è fermato;
   - il run loop registra frame successivi al trap (frame_no avanza).
2. **GATE di non-regressione**: ri-eseguire il GATE Blitz
   (`run_gate_demo`, `boot/phases/interrupts.rs:241-247`): la tabella
   `avg_resolve_ms` a 6400 nodi deve restare entro il **5 %** della baseline
   pre-epoch (28 ms) — misura l'overhead dell'istrumentazione su codice Stylo
   reale. Il viewer (2 frame pilotati) NON deve trappare con i deadline scelti.
3. **Nessun trap spurio**: `make run-test CARGO_FEATURES=boot-checks` verde
   (hello, spike, component, egui-demo, gate, viewer) — copre il censimento §5.
   NB (memo run-test): la feature va passata a `run-test`, non solo a `iso`.
4. **Manuale**: `make run`, aprire 3+ finestre, lanciare lo spinner dal
   launcher: la finestra sparisce, le altre restano interattive; verificare il
   log via seriale (o netconsole su HW reale).

## Fasi di implementazione

**Fase 1 — kernel + precompiler (un commit, atomico):**
- `kernel/src/wasm/wt/mod.rs`: `epoch_interruption(true)` in `engine_config`;
  `ENGINE` promossa a static di modulo; `pub fn epoch_tick()`; deadline ∞ in
  `run_cwasm`/`run_hello`.
- `kernel/src/wasm/wt/wm.rs`: costanti deadline; riarmo per-call in
  `frame_all`/`run_initialize`/`extract_manifest`/`run_reactor_spike`;
  match su `Trap::Interrupt` + log `WATCHDOG`; `run_initialize` → esito.
- `kernel/src/wasm/wt/component.rs`: deadline ∞ allo store di bring-up.
- `kernel/src/timer.rs`: ramo BSP di `timer_handler` → `wt::epoch_tick()`.
- `tools/wt-precompile/src/main.rs`: `epoch_interruption(true)`.
- `tools/wt-spin-reactor/` + boot-check spinner (`boot/phases/interrupts.rs`).

**Fase 2 — re-AOT artefatti (stesso treno di commit della fase 1):**
- rigenerare tutti i blob embedded (`kernel/src/wasm/wt/*.cwasm`),
  `apps/*.cwasm`, artefatti SDK; `make iso` completo; nota migrazione per
  `/mnt/apps` in `apps/README.md` + `docs/api/README.md`.

**Fase 3 — policy UI (separata, dopo che 1+2 sono verdi):**
- notifica utente alla kill (toast del desktop shell via `wm.window_list`
  esteso o nuova host fn di notifica — richiede aggiornamento `docs/api/`);
  eventuale placeholder "l'app non risponde" al posto della chiusura secca;
  eventuale kill cooperativo dei tool CLI (`epoch_deadline_callback` +
  `Continue(1)` come kill-point periodico su flag Ctrl-C) — oggi fuori scope.

## Questioni aperte

- **Overhead reale** dell'istrumentazione su AOT giganti (viewer ~63 MB di
  Stylo+vello): il GATE decide; nessun numero pubblico è affidabile per questo
  workload.
- I valori 6/50/100 tick sono ipotesi ragionate, da tarare su HW reale (il
  primo frame del viewer su macchine lente potrebbe richiedere più di 0,5 s).
- Fase 3: serve davvero il placeholder, o la chiusura + log basta per un OS da
  sviluppatori? Decidere quando il watchdog avrà ucciso qualcosa di vero.
