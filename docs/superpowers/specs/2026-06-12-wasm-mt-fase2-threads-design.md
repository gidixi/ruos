# Fase 2 — wasm-threads MVP: fiber cooperativi M:N (design)

**Data:** 2026-06-12
**Stato:** ✅ IMPLEMENTATO (2026-06-12, changelog 486-493 — esiti in fondo, §13)
**Prerequisito:** Fase 1 (compositor parallelo + audit host fn) conclusa — changelog 476.
**Ri-specifica** l'outline §Fase 2 di
`docs/superpowers/specs/2026-06-12-wasm-multithreading-roadmap-design.md`.

## Obiettivo

`std::thread` e rayon funzionanti nelle app `.cwasm` compilate
`wasm32-wasip1-threads`, con i thread implementati come **fiber cooperativi**
schedulati **M:N** sui core ComputeApp. Nessuno scheduler preemptive: un fiber
cede il core SOLO a `atomic.wait`, a una host-call bloccante, o al return. La
sandbox resta il runtime WASM. SMP-single-app: i thread sono di UNA app che
condivide una `SharedMemory`.

### Perché fiber M:N (modello B) e non core pinnati (modello A)

Il pivot vieta lo scheduler preemptive. Un fiber cooperativo NON è preemptive
(cede solo ai punti di yield), quindi è l'estensione naturale di "concurrency =
async cooperative". Conseguenze:

- **Thread parcheggiato costa ZERO core** (fiber sospeso, non un core in `hlt`).
- **Parallelismo vero fino a `num_core`**: ≤N thread runnable ⇒ ognuno un core
  (identico al pinnato). rayon dimensiona di default a `num_core` → caso ideale.
- **Oversubscription cooperativa gratis**: >N thread → gli extra avanzano quando
  i running si parcheggiano.
- **Interazione naturale col compositing di Fase 1**: i core ComputeApp fanno
  work-stealing su {job compositing, task executor, fiber-thread runnable}. Le
  app che crunchano prendono core al compositing, che degrada via il fallback
  inline già esistente. Nessuna logica di "lending" speciale.

Limite accettato e documentato: un thread **puro-CPU che non si parcheggia mai**
monopolizza il suo core (cooperativo, non preempt). rayon/std normalmente si
parcheggiano (aspettano lavoro/lock), quindi non è il caso reale.

**Niente fallback al modello A (core pinnati).** Ci si impegna su B: se il
bring-up gate §1 punto (3) fallisce, NON si ripiega su A — si **risolve**
`async_support` in no_std (vedi §1). Il piano si blocca su quel punto finché
non funziona.

---

## 1. Bring-up gate (PRIMO step del piano, gating HARD)

Esperimento isolato `bringup-threads.cwasm` (stile WT-COMPONENT-OK di Fase 1) che
verifica **3 punti** PRIMA di scrivere il resto. Marker boot-check
`THREADS-OK <n>` per ciascuno.

1. **Atomics + SharedMemory** nell'engine Wasmtime no_std del kernel: un modulo
   con `(memory 1 1 shared)` + `i32.atomic.rmw.add` gira e il `LOCK XADD`
   modifica la shared memory (rilettura del valore lato host).
2. **`wasi_thread_spawn`**: istanziare lo STESSO modulo condividendo la
   `SharedMemory`, eseguire `wasi_thread_start(tid, arg)`, e osservare il
   secondo "thread" scrivere una cella che il primo rilegge cambiata.
3. **`atomic.wait` sospende un fiber** (`Config::async_support` in no_std): un
   `atomic.wait` su un valore non ancora cambiato deve **cedere il core** (fiber
   suspend), NON bloccarlo in busy/hlt; un `atomic.notify` da un altro fiber lo
   risveglia e l'esecuzione prosegue.

**Go/no-go.**
- (1) o (2) falliscono → problema di build/feature Wasmtime no_std (proposta
  threads, SharedMemory): si risolve prima di ogni altra cosa.
- (3) fallisce → si **risolve `async_support` in no_std** (indagine sul supporto
  async/fiber di Wasmtime nel nostro build runtime-only; eventuale patch del
  vendoring). NON si cambia modello. Il piano resta bloccato qui finché (3) non
  passa.

Verifica su QEMU `-smp 4 -m 2048` e su **VBox** (regola CPU-sensitive).

## 2. Toolchain & engine

- `rustup target add wasm32-wasip1-threads` in WSL. Le app MT compilano con quel
  target; le app esistenti restano `wasm32-wasip1` (invariate).
- **wt-precompile** (`Config` Cranelift, lato build dei `.cwasm`): abilita la
  proposta threads → atomics compilati a istruzioni x86 `lock`-prefixed;
  `atomic.wait/notify` come libcall verso l'host.
- Lo STESSO flag threads nel `Config` runtime del kernel (engine identico:
  regola del deserialize `.cwasm` già esistente di Fase 1, altrimenti il
  caricamento fallisce).
- Dopo che il gate (3) passa: `async_support(true)` nel `Config` runtime, e
  instanziazione/chiamata guest via le API `*_async` di Wasmtime.

## 3. SharedMemory in no_std

- Feature `threads` di Wasmtime nel build runtime-only del kernel. Le primitive
  di sync (il `parking_spot`/`WaitNotify` di Wasmtime) sono fornite da NOI come
  `custom-sync-primitives`, mappate sullo scheduler fiber (§5).
- La `SharedMemory` è allocata una volta per app, cresce sotto il suo mutex
  interno, e vive nella **WT VA window demand-paged** di Fase 1.

## 4. Memoria condivisa × demand paging (Fase 1)

- La SharedMemory sta nella VA window (`kernel/src/wasm/wt/demand.rs`). Più thread
  su core diversi possono **demand-faultare pagine condivise in concorrenza**:
  `commit_fault`/`map_page` sono già SMP-safe (serializzati da `MAPPER`,
  `AlreadyMapped` gestito quando due core faultano la stessa pagina —
  verificato in Fase 1). **Nessuna modifica al memory manager.**
- Pagina committata da un thread è visibile agli altri: page table unica
  condivisa (niente per-process PT). `map_page` not-present→present non richiede
  TLB shootdown (già così). W^X invariato (codice AOT già mappato).

## 5. Scheduler fiber-thread M:N (cuore)

- **thread wasm = fiber** (stack dedicato) + una `Store` per thread che condivide
  la `SharedMemory` del modulo.
- **Run loop ComputeApp esteso**: `kernel/src/executor/mod.rs::run_core()` (oggi:
  poll task async → drain inbox → drain pool job → hlt) diventa un work-stealing
  loop su **{job compositing Fase 1, task executor, fiber-thread runnable}**. Un
  core prende un fiber runnable dalla run-queue e lo **esegue finché non cede**
  (park / host-call bloccante / return), poi passa al prossimo lavoro.
- **`wasi_thread_spawn(start_arg) -> tid`** (host fn): alloca `tid`, crea la
  `Store` figlia condividendo la SharedMemory, crea un fiber che esegue
  l'export `wasi_thread_start(tid, start_arg)`, lo mette nella run-queue dei
  fiber runnable, ritorna `tid`. (Policy spawn: §6.)
- **`atomic.wait(addr, expected, timeout) -> i32`** (libcall): **adaptive spin**
  con `PAUSE` per ~N cicli ricontrollando `mem[addr]`; se ancora `== expected`
  → registra il fiber sulla **wait-queue keyed by `addr`** e **sospende** (cede
  il core). Timeout via deadline `RDTSC`. Ritorni: 0=ok(notified), 1=not-equal,
  2=timeout (semantica wasm threads).
- **`atomic.notify(addr, count) -> i32`** (libcall): trova fino a `count` waiter
  su `addr`, li marca runnable (in run-queue), e manda **IPI** (`VEC_WAKE`,
  `kernel/src/idt.rs`) ai core dormienti perché uno li raccolga. Ritorna il
  numero svegliati.
- **Wait-queue**: mappa `addr -> lista di fiber sospesi`. `#[repr(align(64))]`,
  sharded per ridurre contesa/false-sharing (§9).
- **Parallelismo esposto**: host fn `available_parallelism() -> u32` (= core
  ComputeApp) + env `RAYON_NUM_THREADS`. rayon si dimensiona così.
- **Degrado vs compositing**: quando i fiber-thread di un'app girano, prendono
  core dal work-stealing → meno core per band/frame job → fallback inline di
  Fase 1. Automatico. Osservabile con l'overlay `wm-fps` (changelog wm-fps).

## 6. Policy spawn & limiti

- `available_parallelism` = numero core ComputeApp. Spawn OLTRE = **consentito**
  (oversubscription cooperativa: i fiber extra avanzano quando i running si
  parcheggiano), ma loggato.
- Thread puro-CPU che non si parcheggia mai → monopolizza il suo core. Limite
  documentato del modello cooperativo; non è il pattern di rayon/std.

## 7. Watchdog & kill

- **Epoch deadline per-thread-store** (riusa l'infra epoch di Fase 1). Un thread
  che sfora la deadline o trappa → **muore l'intera app**: i lock guest nella
  shared memory resterebbero avvelenati. Teardown atomico di TUTTI i thread/Store
  dell'app + free della `SharedMemory`. Coerente con "il runtime è la sandbox".
- Teardown multi-core: si marca l'app morente; i core che eseguono suoi fiber li
  abbandonano al prossimo yield; il GUI core fa il reap (come le finestre di
  Fase 1). Nessun core resta a eseguire un guest di un'app morta.

## 8. ps / proc

- Thread visibili in `ps` come `win:foo#tid`. Il registry `proc`
  (`kernel/src/proc.rs`) è esteso per i tid; il fix `proc::REGISTRY`→`IrqMutex`
  di Fase 1 copre già l'accesso concorrente da più core.

## 9. Performance

- **Uncontended atomics = nativo**: Cranelift compila a x86 `lock`-prefixed
  (`LOCK CMPXCHG`/`XADD`). Un mutex non conteso = zero host-call, velocità
  quasi-nativa. È il payoff dell'AOT.
- **wait/notify** (solo in contesa): adaptive spin (`PAUSE`) → suspend fiber →
  notify via IPI.
- **Stato scheduler per-core + `#[repr(align(64))]`** → niente false sharing
  (la leva multicore vera: cache-line ping-pong MESI ucciderebbe lo scaling).
  Run-queue/wait-queue/contatori separati per cache line.
- **`core::hint::likely`/`#[cold]`** sui due hot loop (fast-path atomico, loop
  scheduler) per guidare il code layout (frontend/prefetch). I prefissi x86 di
  branch-hint sono morti sui CPU moderni → non usati.
- Context switch fiber = asm del fiber crate (~decine di ns). `MWAIT` escluso da
  v1 (VM-unsafe: VMEXIT/non esposto su VBox/QEMU); possibile ottimizzazione
  futura solo con core pinnati.
- Non si sfrutta la speculazione (Spectre): cross-thread dentro UNA app non
  cambia il modello (stessa sandbox); non si peggiora.

## 10. Fuori scope (v1)

- Thread nei tool wasmi (l'interprete non ha shared memory; i CLI restano
  single-thread — chi vuole parallelismo scrive un'app `.cwasm`).
- Component model multi-thread.

## 13. Esiti implementazione (2026-06-12)

Implementata task-by-task (piano
`docs/superpowers/plans/2026-06-12-wasm-mt-fase2-threads.md`, changelog
486-493, commit su main da 9df5003 in poi). Tutti i gate verdi:

- `THREADS-OK 1` (SharedMemory + atomics no_std), `THREADS-FIBER-OK`
  (suspend/resume cross-core), `THREADS-OK 3` (wait sospende il fiber, notify
  IPI), `THREADS-OK 2` (thread-spawn = fresh Instance su stessa memoria) — su
  QEMU `-smp 4` E `-smp 1` (fallback BSP). Test: `make run-threads-test`.
- End-to-end reale: `parsum` (rayon, `PARSUM_OK threads=N`) e `mtstress`
  (Mutex std conteso, `STRESS_MT_OK count=400000` esatto) su
  `wasm32-wasip1-threads`; `mtstress trap` → kill-group exit 134, shell viva.

Deviazioni dalla spec (documentate nei changelog):

1. **Niente `async_support`** (§1 punto 3): wasmtime resta sync; ogni guest
   threaded gira in un fiber NOSTRO (`wasmtime-internal-fiber`, backend
   no_std). L'hook futex sospende il fiber — stesso requisito comportamentale,
   meno superficie.
2. **Epoch deadline dei thread store = `NO_DEADLINE_TICKS`** (§7): una
   deadline assoluta trapperebbe al resume dopo un park lungo. Il kill-group
   su trap resta (un thread che trappa uccide il gruppo: runnable al take,
   parcheggiati via `kill_group_waiters`, in-esecuzione al prossimo park).
3. **Pre-filtro timeout = contatore `TIMED_WAITERS`**, non `EARLIEST_DEADLINE`
   atomico (race insert-vs-rescan con deadline perse).
4. **Anti-lost-wakeup = crediti per-shard** (notify che incrocia un park in
   volo lascia un credito consumato da `run_one` prima dell'insert in WAITQ);
   al più un wake spurio per chiave — il chiamante futex ricontrolla.
- `MWAIT`. Scheduler preemptive (Fase 3, solo carta).

## 11. Test & verifica

- **Bring-up gate marker** (§1): `THREADS-OK 1`, `THREADS-OK 2`, `THREADS-OK 3`.
- **`parsum.cwasm`**: somma parallela rayon su un array (CPU-bound, ≤core
  thread) → risultato corretto + speedup misurato vs single-thread (`RDTSC`).
- **Stress mutex conteso**: N thread incrementano un contatore condiviso M volte
  sotto un `Mutex` → valore finale esatto (esercita atomics + wait/notify +
  shared memory + teardown).
- **Regressione**: `run-test`, `frame-smp-test`, `comp-smp` verdi (i thread non
  rompono il compositing). **VBox obbligatorio** (`-smp 4+ -m 2048`).
- Overlay `wm-fps` / marker `frame cores`: il compositing degrada ma non si
  congela mentre un'app usa thread.

## 12. Rischi

- **#1 — `async_support` Wasmtime no_std** (gate §1.3). Niente fallback: se cade,
  si risolve in no_std (indagine/patch del vendoring), il piano si blocca lì.
- Memoria fiber: ogni thread uno stack → bounded dal numero di thread.
- Debug concorrenza bare-metal: netconsole + marker + overlay `wm-fps`, come
  Fase 1. Verifica CPU-sensitive sempre su VBox.

## File presumibilmente toccati (orientativo, non esaustivo)

- `kernel/src/wasm/wt/mod.rs` / `precompile`: `Config` threads + `async_support`.
- `kernel/src/wasm/wt/threads.rs` (nuovo): scheduler fiber, run-queue,
  wait-queue, `wasi_thread_spawn`, `atomic.wait/notify` libcall,
  `available_parallelism`.
- `kernel/src/executor/mod.rs`: `run_core` esteso (work-stealing fiber).
- `kernel/src/proc.rs`: tid in `ps`.
- `kernel/src/wasm/wt/bringup_threads.*` (nuovo): bring-up gate guest + marker.
- App test: `parsum`/stress (path da concordare, workflow new-ruos-app).
- Toolchain WSL: `wasm32-wasip1-threads`.
