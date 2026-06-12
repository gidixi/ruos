# Multithreading per app WASM — Roadmap + spec Fase 1

**Data:** 2026-06-12
**Stato:** approvato (brainstorming), Fase 1 da pianificare — NON ancora implementata
**Aggiorna il pivot 2026-05-28:** il drop "niente preemptive thread scheduler"
resta; il MT arriva per gradi SENZA scheduler (thread = core dedicato), con il
preemptive documentato come fase futura non costruita.

## Obiettivo

Le app moderne usano il multithreading. ruos oggi lo preclude: guest
single-thread (niente wasm-threads), concorrenza solo cooperativa, parallelismo
solo kernel-side (band compositing, compute pool). Traguardo: **`std::thread` e
rayon funzionanti nelle app `.cwasm`** (target `wasm32-wasip1-threads`), senza
buttare il modello di sicurezza (sandbox WASM al posto di ring 3) e senza
costruire uno scheduler preemptive.

Decisioni dal brainstorming:

- Casi d'uso target: TUTTI (calcolo parallelo dati, crate esistenti con
  std::thread/rayon, app reattive UI+background, server concorrenti).
- Hardware target: PC reale 4-16 core (VBox 6 vCPU per i test).
- Densità: thread = core dedicato basta (pool rayon dimensionato sui core);
  niente oversubscription in v1/v2.
- Sequenza scelta: **audit-first** — prima si rende il kernel sicuro alle
  chiamate host concorrenti (Fase 1, che da sola dà già un beneficio visibile),
  poi si abilitano i thread veri (Fase 2). Motivo: debuggare UNA incognita
  alla volta; la Fase 2 atterra su host fn già blindate.

## Roadmap

| Fase | Cosa | Beneficio | Stato |
|---|---|---|---|
| **1** | Compositor parallelo: `frame()` delle finestre sveglie su core diversi del compute pool + **audit rientranza host fn** | un'app lenta non blocca più il desktop; kernel pronto alla concorrenza guest | spec QUI sotto, implementabile |
| **2** | wasm-threads MVP: atomics + SharedMemory + `wasm32-wasip1-threads` + `wasi_thread_spawn` (thread = core dedicato, `atomic.wait` = park hlt/IPI) | `std::thread` e rayon funzionano nelle app | outline §Fase 2, da rispecificare a Fase 1 conclusa |
| **3** | Oversubscription / scheduler preemptive | densità di thread > core | FUTURO, documentato, NON costruito |

Criteri go/no-go Fase 1 → Fase 2: run-test + comp-smp verdi col compositor
parallelo; checklist audit completata al 100%; stress test host-fn concorrenti
verde su VBox (regola progetto: cambi CPU-sensitive si verificano su VBox).

### Cosa NON faremo (in nessuna fase di questa roadmap)

- Scheduler preemptive ORA (Fase 3 = solo carta finché non c'è domanda reale
  di densità: il giorno che servono più thread che core, si rispecifica).
- MT nei tool wasmi (l'interprete non ha shared memory; i CLI restano
  single-thread — chi vuole parallelismo scrive un'app `.cwasm`).
- API worker-a-messaggi dedicata (ridondante: rayon in Fase 2 copre il caso
  calcolo; scartata nel brainstorming per YAGNI).
- ring 3 / page table per processo: la sandbox resta il runtime WASM.

---

# Fase 1 — Compositor parallelo + audit rientranza (SPEC)

## Idea

Wasmtime già lo consente: `Engine`/`Module` sono `Send+Sync`, Store diversi
possono girare su core diversi. Oggi è `frame_all()` che serializza i `frame()`
di tutte le finestre sul core GUI — per scelta, non per limite. La Fase 1
dispatcha i `frame()` delle finestre sveglie come job sul compute pool SMP
(stesso pool del band compositing), con join prima di `present()`.

Effetto collaterale VOLUTO ed essenziale: le host fn (`wm`/`sys`/`term`/`net`/
`wasi`) vengono chiamate da più core contemporaneamente → ogni accesso a stato
kernel globale dentro un `func_wrap` va auditato e, dove serve, corretto. È il
prerequisito di tutto il MT successivo, fatto su un sistema ancora
single-thread-per-app (bug riproducibili).

## 1. Architettura del dispatch

`kernel/src/wasm/wt/wm.rs`, `frame_all()`:

- **Selezione** (sul core GUI, invariata): `compute_awake` decide quali
  finestre girano questo giro; deadline epoch armata per-store PRIMA del
  dispatch (come oggi).
- **Dispatch**: per ogni finestra sveglia, un job sul compute pool che esegue
  `inst.get_typed_func::<(), ()>("frame")` + `frame.call(...)` + la gestione
  errori/causa crash ESISTENTE (publish APP_CRASHED, `close_requested`).
  Pattern di handoff = quello del band compositing: arena statica di
  descrittori (puntatori raw a `Window`), il GUI core riempie `[0, n)`,
  submitta, e **blocca sul join prima di toccare di nuovo `wins`** — nessun
  aliasing (ogni job riceve UNA finestra distinta, il GUI core non legge né
  muta `wins` durante il volo).
- **Pool pieno / 1-2 core**: fallback inline sul core GUI (stesso pattern di
  `dispatch_bands`) — il sistema degrada al comportamento attuale.
- **Sezioni che RESTANO seriali sul core GUI**: spawn/instantiate/
  `_initialize`, reap, richieste deferred (bg/overlay/move/minimize/activate),
  input routing, drain_kevents, present. Solo l'esecuzione dei `frame()` va
  in parallelo.
- **Post-join** (invariato): lettura `committed`/`win_w/win_h`, adozione size,
  `framed_once`, present.

### Feature gate

Cargo feature **`wm-serial-frames`**: ripristina il loop seriale attuale
(baseline per bisect, stesso precedente di `serial-composite`). Default =
parallelo.

### Vincoli di correttezza

- `Store<AppState>`: acceduto SOLO dal job della sua finestra durante il volo
  (il descriptor è `*mut Window`; il job è l'unico owner fino al join).
  `AppState` deve essere `Send` (verificare: contiene fd table, WmState,
  pixels — tutti dati owned; eventuali `Rc`/non-Send vanno bonificati).
- Epoch watchdog: il ticking è globale all'engine, le deadline sono per-store
  → funziona invariato per N store in parallelo. Il kill di un guest che sfora
  resta per-finestra.
- W^X / pagine codice: gli AP eseguono codice AOT già mappato (nessuna
  instantiate sugli AP) — nessun cambiamento al memory management.

## 2. Audit di rientranza host fn (il cuore della fase)

Metodo: censimento di OGNI `func_wrap` raggiungibile da una finestra
(`wm.rs`, `sys.rs`, `term.rs`, `net.rs`, `wasi.rs`, `gui.rs` se applicabile) →
per ciascuna, elenco dello stato GLOBALE toccato → verifica lock discipline →
fix o annotazione. Deliverable: doc separato
`docs/superpowers/specs/<data-audit>-hostfn-reentrancy-audit.md` (datato al
giorno in cui l'audit viene eseguito) con una riga di esito per ogni fn.

Aree note da auditare (lista di partenza, NON esaustiva — il censimento è
parte del lavoro):

| Area | Stato condiviso | Rischio atteso |
|---|---|---|
| `wm.spawn/window_list/app_list` | `WINDOW_SNAPSHOT`, catalogo app | lock già presenti, verificare ordine |
| `wm.poweroff/reboot/power_*` | `power::PENDING` (IrqMutex) | basso (IrqMutex, no nesting) |
| `sys.events_poll` | kevent BUS (IrqMutex) + cursore per-store | basso |
| `sys.cpustat/proc_stat/meminfo` | proc registry (spin), cpustat | medio: alloc sotto lock? |
| `term.read/write/resize` | PTY ring (spin) condivisi con fiber BSP | **alto**: due lock-holder su core diversi, verificare no doppio-lock e no spin lunghi |
| `wasi fd_*` | fd table per-store ✓ ma VFS globale sotto | medio: `vfs::block_on` da un job AP — semantica di polling fuori dall'executor da verificare |
| `net.*` | NET mutex | **alto**: regola esistente "mai tenere NET attraverso wait" — verificare che valga da AP |
| klog/binfo | try_lock | basso (già panic-safe) |
| `gfx::push/pop/geom` | IrqMutex/atomics | basso |

Regole d'oro da far rispettare (e documentare nel doc di audit):

1. Nessun `func_wrap` tiene DUE lock kernel annidati senza ordine globale
   documentato.
2. Nessuno spin-lock tenuto attraverso un'operazione O(n) non bounded
   (alloc grosse, copy di buffer interi) se contendibile da IRQ o altro core.
3. Stato per-finestra vive in `WmState`/`AppState` (per-store), MAI in static
   globali indicizzati implicitamente "dalla finestra corrente".

## 3. Telemetria e test

- **Boot-check marker** (pattern `composite cores=`): con
  `CARGO_FEATURES=boot-checks`, il compositor logga dopo N frame i core
  distinti che hanno eseguito job `frame()`:
  `frame cores=K [c1, c2, ...]` — il test asserisce K ≥ 2 con ≥ 2 finestre
  sveglie e ≥ 4 vCPU.
- **Stress test host-fn**: app di test (reactor) che martella `sys.proc_stat`
  + `term` write in loop su due finestre parallele per M frame; il marker
  verifica niente deadlock/panic (timeout = fail).
- `make run-test` e il test comp-smp esistente restano verdi (il band
  compositing condivide il pool: verificare che frame-jobs + band-jobs si
  spartiscano i core senza starvation — i frame job vengono joinati PRIMA del
  present, quindi non si sovrappongono alle bande per costruzione).
- **VBox**: verifica obbligatoria (cambio CPU-sensitive; regola progetto).
- Con feature `wm-serial-frames`: tutto identico a oggi (regressione).

## 4. Parametri Fase 1

| Parametro | Valore |
|---|---|
| Max frame-job in volo | min(finestre sveglie, core compute liberi) |
| Pool | lo stesso compute pool SMP esistente (nessun pool nuovo) |
| Fallback | inline sul core GUI (pool pieno o ≤ 2 core) |
| Feature gate | `wm-serial-frames` (ripristina il serale) |

---

# Fase 2 — wasm-threads MVP (OUTLINE, da rispecificare)

Prerequisito: Fase 1 conclusa (audit 100%, stress verdi).

Componenti previsti:

1. **Toolchain**: `rustup target add wasm32-wasip1-threads` in WSL; le app MT
   compilano con quel target (le app esistenti restano `wasm32-wasip1`).
2. **wt-precompile**: abilitare la proposta threads nel `Config` Cranelift
   (atomics → istruzioni lock-prefixed x86; `atomic.wait/notify` → libcall).
   Lo stesso flag va nel `Config` runtime kernel (engine identico, regola
   esistente del deserialize).
3. **SharedMemory no_std**: feature `threads` di Wasmtime nel nostro build
   runtime-only; le primitive di sync sono le `custom-sync-primitives` già
   fornite. RISCHIO PRINCIPALE della fase: mai esercitata in no_std — il
   bring-up gate (stile WT-COMPONENT-OK) va fatto PRIMA di tutto il resto.
4. **`wasi_thread_spawn(start_arg) -> tid`** (host fn): crea una nuova
   `Instance` dello STESSO modulo che condivide la SharedMemory, e la esegue
   (`wasi_thread_start(tid, start_arg)`) **run-to-completion su un core
   dedicato del compute pool**. Spawn oltre i core liberi → errore (rayon va
   dimensionato: si espone il parallelismo disponibile via env
   `RAYON_NUM_THREADS`/host fn dedicata — dettaglio da spec Fase 2).
5. **`atomic.wait` = park**: il core del thread va in hlt; `atomic.notify`
   sveglia via IPI (precedente: wake del pool). Niente scheduler: un thread
   parcheggiato OCCUPA il suo core ma non lo brucia.
6. **Watchdog & kill policy**: deadline epoch per-thread-store; un thread
   killato (deadline o trap) → **muore l'intera app** (i lock guest resterebbero
   avvelenati; coerente con "il runtime è la sandbox").
7. **`ps`/proc**: thread visibili come `win:foo#tid`.

Fuori scope Fase 2: thread nei tool wasmi, component model multi-thread,
oversubscription.

# Fase 3 — Preemptive (FUTURO, non costruito)

Solo carta: context switch su timer IRQ, stack kernel per-thread, scheduler
con priorità. Si apre SOLO se emerge domanda reale di densità (> core-2 thread
contemporanei). Il pivot resta valido fino ad allora.

# Rischi trasversali

- **Multi-tenant** (obiettivo dichiarato del progetto): più superficie
  concorrente = più superficie di audit; la Fase 1 È la mitigazione (audit
  prima dei thread). Spectre: la superficie cross-thread dentro UNA app non
  cambia il modello (stessa sandbox); cross-app resta come oggi.
- **Debugging**: bug di concorrenza su bare-metal — investire nei marker
  boot-check e nello stress test PRIMA di abilitare i thread (Fase 1 li
  costruisce).
- **wasmi**: divergenza permanente tool CLI (ST) vs app (MT) — accettata e
  documentata in docs/api.
