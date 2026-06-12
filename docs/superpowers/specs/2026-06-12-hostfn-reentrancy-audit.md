# Audit di rientranza host fn `wt/*` — MT Fase 1

**Data:** 2026-06-12
**Scopo:** prima della Fase 1 (compositor parallelo) ogni `frame()` di finestra
girava sul solo core GUI. Con il dispatch parallelo, le `frame()` delle finestre
sveglie girano su core diversi del compute pool SMP → ogni host fn `wt/*`
raggiungibile da `frame()` può essere chiamata **da più core insieme**. Questo
doc censisce quelle host fn e lo stato globale che toccano, e dà un esito
(SAFE / FIX-REQUIRED) per ciascuna.

**Scope:** SOLO i runtime Wasmtime `wt/*` (`kernel/src/wasm/wt/{wm,sys,term,net,
gfx,gui,component}.rs`). Le `kernel/src/wasm/host/*` NON sono in scope: sono il
runtime **wasmi** (tool CLI single-thread), NON raggiungibili da una finestra
GUI Wasmtime.

**Bring-up gate (Task 0, prerequisito hard):** verificato il 2026-06-12 in QEMU
(`-smp 4`, init `compositor-init.sh`) che:
- una `frame()` di guest Wasmtime **gira su un AP** (`PROBE frame() ran on core=2`,
  core 2 = ComputeApp), non solo sul BSP — il fault/trap routing del guest
  funziona off-BSP;
- un trap del guest su un AP è gestito (`PROBE call err=true interrupt=true` con
  deadline forzato a 1 tick) senza panicare il kernel; boot prosegue fino a shell.

Verifica VBox (regola progetto, cambio CPU-sensitive): **DA FARE manualmente**
(QEMU verde; ISO `make iso CARGO_FEATURES=boot-checks`, ≥4 vCPU).

---

## Modello di concorrenza (perché le tre fasi del loop contano)

`Compositor::frame_all` è ora in 3 fasi (`kernel/src/wasm/wt/wm.rs`):

- **Fase A (core GUI, seriale):** `compute_awake` + arming deadline + riempimento
  arena `*mut Window`.
- **Fase B (parallela):** `dispatch_frames` esegue le `frame()` come job sul
  compute pool, **join prima di tornare**. Qui — e SOLO qui — le host fn `wt/*`
  sono chiamate da più core.
- **Fase C (core GUI, seriale, post-join):** adozione committed size + `framed_once`.

Conseguenza chiave per l'audit: tutto ciò che il run loop fa FUORI da Fase B —
`reap`, `refresh_app_catalog`, input routing (`gfx::fold_mouse`/`pop`),
`drain_kevents`, la fase deferred (spawn/bg/overlay/move/minimize/activate),
publish `WINDOW_SNAPSHOT`/`APP_CATALOG`, `present` (incl. `dispatch_bands`) —
NON è concorrente con le `frame()`. I writer di `WINDOW_SNAPSHOT`/`APP_CATALOG`
(fase deferred) non corrono mai con i loro reader (host fn in Fase B): fasi
diverse del loop. I frame job possono però **leggerli in concorrenza tra loro**,
e possono toccare in concorrenza primitivi condivisi con task su ALTRI core (es.
`add_cpu_tsc` dall'executor, ring PTY con la fiber shell sul BSP).

## Regole d'oro (da far rispettare)

1. Nessun `func_wrap` tiene DUE lock kernel annidati senza ordine globale documentato.
2. Nessuno spin-lock tenuto attraverso un'operazione O(n) non bounded (alloc grosse,
   copy di buffer interi) se contendibile da IRQ o altro core.
3. Stato per-finestra in `WmState`/`AppState` (per-store), MAI in static globali
   indicizzati "dalla finestra corrente".

---

## Esiti per stato condiviso

| # | Host fn (modulo.fn) | Stato condiviso | Primitivo (file:riga) | Op sotto lock | Esito |
|---|---|---|---|---|---|
| 1 | `term.read/write/resize/open/close` | ring PTY `PAIRS[idx]` | `spin::Mutex` (`pty/mod.rs:19`) **wrappato in `without_interrupts`** ai call site (`master_output_try` :188, `master_output_len` :202, `master_input_push` :137) | O(1) pop/push/len; ldisc bounded | **SAFE** — il `without_interrupts` dà parità interrupt-safe a IrqMutex; sezioni O(1); `SLAVE_RX` è ring SPSC lock-free, `SLAVE_WAKER` è `IrqMutex` |
| 2 | `gfx.poll_event/pending/blit`, `wm.start_move` (`mouse_pos`) | coda `EVENTS`, `CUR_LOCK`, framebuffer/geom/mouse | `IrqMutex` (`gfx/mod.rs:248`, `:377`) + atomics (`:19-49`, `:277-281`) | `pop/len/push` O(1); `blit` memcpy lock-free su righe disgiunte; cursor 12×19 sotto `CUR_LOCK` | **SAFE** — tutto atomic o IrqMutex; blit senza lock, righe disgiunte (band dispatch) |
| 3 | `net.resolve_start/poll/dial/read/write/close` | `RESOLVES`, `NET` | `RESOLVES` = `IrqMutex` (`wt/net.rs:44`); `NET` = `spin::Mutex` (`net/mod.rs:47`) | `RESOLVES` O(1), Vec bounded da pool_size=8; `dns_task` fa l'await DNS **fuori** dal lock (`wt/net.rs:47-57`); `NET` non raggiunto da `frame()` | **SAFE** — `RESOLVES` IrqMutex, slot scritto sotto lock sia da guest sia da `dns_task`; await fuori dal lock; `NET` solo da `net_poll_task` sul BSP |
| 4 | `wm`-crash path → `kevent::publish_named`; `sys.events_poll` → `read_since` | bus `BUS`, `SEQ` | `IrqMutex` (`kevent.rs:64`) + `AtomicU64` | array fisso + `heapless::String<32>` O(1); `read_since` loop bounded da buffer caller | **SAFE** — publish da AP ok (IrqMutex), sezioni O(1) |
| 5 | `wm.power_pending/power_cancel/poweroff/reboot` | `power::PENDING` | `IrqMutex` (`power.rs:32`) | O(1) (`.lock().take()`, deref); publish/spawn fuori dal lock | **SAFE** — nessun nesting, enforce task sul BSP |
| 6 | `sys.proc_stat/proc_list` → `proc::list` | `proc::REGISTRY` | era `spin::Mutex` (`proc.rs:29`) | **clona l'intera `BTreeMap` (alloc String) SOTTO lock**, contendibile da `add_cpu_tsc` su altri core | **FIX-REQUIRED → FIXED** (vedi sotto) |
| 6b | `sys.cpustat` → `cpustat::read` | `CORE[]` | atomics (`sched/cpustat.rs:27`) | lock-free | **SAFE** |
| 7 | `wm.window_list/app_list` | `WINDOW_SNAPSHOT`, `APP_CATALOG` | `IrqMutex` (`wm.rs:1094`, `:185`) | serializzazione bounded (≤~10 finestre / ~100 app); writer in fase deferred (non concorrente) | **SAFE** — IrqMutex; reader-in-Fase-B vs writer-in-deferred = fasi diverse; reader concorrenti tra loro ok |
| 8 | `wm.commit/tick/close/stay_awake/spawn/...` | `WmState`/`AppState` per-store | nessuno (per-store) | — | **SAFE** — stato per-finestra, owner unico = job della finestra durante il volo (regola d'oro #3 rispettata) |

---

## FIXES REQUIRED

### FIX-1 — `proc::REGISTRY`: bare `spin::Mutex` → `IrqMutex` (APPLICATO)

**Problema.** `proc::list()` (`kernel/src/proc.rs:62`) fa
`REGISTRY.lock().values().cloned().collect()` — clona l'intera `BTreeMap<u32,
ProcInfo>` (ogni `ProcInfo` ha un `String name` → alloc) **tenendo il lock**.
`REGISTRY` era un bare `spin::Mutex` (`proc.rs:29`), **senza** `without_interrupts`
ai call site (a differenza delle ring PTY). Raggiungibile da `sys.proc_stat`
dentro `frame()`, ora su un core del pool, mentre l'executor su un ALTRO core
chiama `add_cpu_tsc`/`register`/`unregister` sullo stesso `REGISTRY`. È una
violazione letterale della **regola d'oro #2** (spin-lock tenuto attraverso una
copia O(n) con alloc, contendibile da altro core) e, essendo non
interrupt-masked, fragile a una futura reentry da IRQ.

Oggi NON deadlocka (nessun IRQ handler locka `REGISTRY`; la contesa cross-core è
solo spin bounded da ~decine di processi), ma è esattamente il tipo di stato che
la Fase 1 deve blindare prima di abilitare i thread veri (Fase 2).

**Fix applicato.** `REGISTRY` convertito a `crate::sync::IrqMutex` (interrupt-
masked, stessa API `.lock()`, allineato al resto dello stato hot del kernel).
La sezione critica O(n) ora maschera gli interrupt sul core che la tiene → niente
reentry da IRQ; `n` è piccolo (decine), impatto latenza trascurabile.

- File: `kernel/src/proc.rs:11` (`use spin::Mutex` → `use crate::sync::IrqMutex`),
  `:29` (`static REGISTRY: IrqMutex<...>` + commento di motivazione).
- Verifica: build `make iso` OK; boot QEMU `-smp 4` OK (parallelo e seriale).
- ABI app-facing invariata (nessun aggiornamento `docs/api/` necessario).

Nessun altro FIX richiesto: tutte le altre host fn `wt/*` usano già `IrqMutex`/
atomics/`without_interrupts` o stato per-store.

---

## Note

- `proc::REGISTRY` è l'unico stato cross-core raggiungibile da `frame()` che non
  fosse già IrqMutex/atomic. Le ring PTY (`spin::Mutex`) sono di fatto sicure
  perché ogni accessor le wrappa in `without_interrupts` — equivalente a IrqMutex.
- Regola esistente "mai tenere `NET` attraverso una wait": rispettata da AP —
  `frame()` non tocca mai `NET` (solo `RESOLVES`/socket non-bloccanti); il poll
  smoltcp resta sul `net_poll_task` del BSP.
- Riferimenti: design Fase 1 in
  `docs/superpowers/specs/2026-06-12-wasm-multithreading-roadmap-design.md`;
  piano in `docs/superpowers/plans/2026-06-12-wasm-mt-fase1-compositor-parallelo.md`.
