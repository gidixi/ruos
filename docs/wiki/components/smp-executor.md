# SMP / Executor async

> **Stato:** bozza
> **Aggiornato:** 2026-06-10
> **Fonti:** `kernel/src/executor/`, `kernel/src/smp/`, `kernel/src/cpu/`,
> `kernel/src/sched/`
> **Spec collegate:** `docs/superpowers/specs/2026-06-05-smp-shared-nothing-architecture-design.md`

## Cos'è

La concorrenza di ruOS è **cooperativa async su un core, con offload SMP su gli
altri**:

- Il **BSP** (Bootstrap Processor) esegue l'unico executor async (`embassy`),
  tutti i task cooperativi (shell, SSH, net poll, USB poll), e tutto l'I/O.
- Gli **AP** (Application Processor, i core extra) sono un **pool di compute
  offload**: eseguono job puri CPU senza I/O, senza lock, senza accesso al VFS.
  Nessun AP tocca mai l'executor, il runtime WASM, o i device.

Non c'è preemption: un task che non yielda (`.await`) monopolizza il core fino
all'esaurimento del fuel. Il timer IRQ a 100 Hz è il **wake source** che risveglia
l'executor dal `hlt`.

## Dove vive

| File / cartella | Ruolo |
|-----------------|-------|
| `kernel/src/executor/mod.rs` | Executor `embassy-executor`, task spawner, waker |
| `kernel/src/smp/mod.rs` | SMP bring-up, AP entry, cpu_id mapping |
| `kernel/src/smp/pool.rs` | Compute pool: `submit`, `take`, `poll_done` |
| `kernel/src/cpu/mod.rs` | Per-CPU GDT/TSS/IDT, trampoline AP |
| `kernel/src/sched/cpustat.rs` | CPU accounting TSC: busy/idle per core |
| `kernel/src/timer.rs` | LAPIC timer 100 Hz, sleep/delay |

## Modello

```
  BSP (core 0)                          AP 1, 2, …, N-1
  ┌─────────────────────┐               ┌───────────────────┐
  │ embassy executor     │               │ hlt-idle worker    │
  │  ├─ shell fiber      │               │  loop:             │
  │  ├─ SSH fiber        │      submit   │   take(job)        │
  │  ├─ net poll         │  ──────────►  │   run fn(&[u8])    │
  │  ├─ USB poll         │               │   signal done      │
  │  └─ GUI compositor   │  ◄──────────  │   hlt until next   │
  │                      │   poll_done   └───────────────────┘
  │ timer IRQ 100 Hz     │
  │ → wake sleepers      │
  └─────────────────────┘
```

## Executor async

L'executor è un **`embassy-executor`** single-CPU:

- **Spawn**: i task sono spawned come `#[embassy_executor::task]` con waker
  personalizzati.
- **Poll loop**: il BSP chiama `executor::run()` (mai ritorna). Quando non ci sono
  task pronti, `hlt` fino alla prossima IRQ (timer o input).
- **Wake**: il timer IRQ a 100 Hz (`timer.rs`) scorre i waker registrati e sveglia
  quelli scaduti. Un evento esterno (keystroke, pacchetto rete) sveglia il task
  appropriato.

### Fiber vs task async

I task WASM non sono task async nativi — sono **fiber** (`wasm/fiber.rs`). La
fiber è un task async che incapsula un'esecuzione WASM sincrona e la sospende/
riprende. Dall'esterno sembra un task async (ha un waker, è polled dall'executor);
dall'interno è un'esecuzione lineare che "blocca" su host call.

## SMP: compute pool

### Bring-up

Alla fase 3 del boot (`interrupts`), il kernel legge la **MP response** di Limine
e porta ogni AP online:

1. L'AP esegue il trampoline (`cpu/mod.rs`): carica la propria GDT/TSS/IDT (ogni
   core ha la sua copia).
2. Riceve un `cpu_id` denso (0 = BSP, 1 = primo AP, …).
3. Si parcheggia in `hlt` nel worker loop.

L'identità del core si legge dal **LAPIC ID** (non da `gs:[0]` — workaround per
VirtualBox che non setta GSBASE sugli AP).

### Job pool

`smp/pool.rs` espone un'interfaccia simple di offload:

- **`submit(fn(&[u8]) -> u64, data: &[u8])`** — pubblica un job. Il job è una
  funzione pura: niente I/O, niente captures, niente lock. Riceve un buffer
  dati immutabile e ritorna un `u64`.
- **`take()`** — l'AP in idle prende un job dalla coda.
- **`poll_done()`** — il BSP raccoglie i risultati completati.

I job sono in slot fissi (nessuna allocazione dinamica nel path caldo). Un IPI
(Inter-Processor Interrupt) sveglia gli AP parcheggiati.

### Uso concreto: compositing parallelo

Il caso d'uso principale dell'SMP è il **compositing a bande** del
[compositor](compositor.md): lo schermo è diviso in bande orizzontali, una per
core online. Ogni banda è un job puro (legge i pixel delle finestre, scrive nel
back-buffer). Le bande sono disgiunte → nessun core tocca lo stesso byte → nessun
lock. Il BSP fa il join e una sola blit finale.

`smptest` (il tool) verifica che il compositing multi-core sia **byte-identical**
al seriale single-core.

### GUI su core dedicato

Il compositor può girare su un **core dedicato**: `gui_worker_loop` (in `smp/`)
attende che il BSP pubblichi i byte del `compositor.cwasm` su una mailbox atomica
(`COMPOSITOR_MAILBOX`), poi esegue `run_compositor_gate` per sempre su quel core.
Il BSP resta libero per net/usb/ssh.

## CPU accounting

`sched/cpustat.rs` tiene contatori per-core basati sul **TSC** (Time Stamp
Counter):

- **busy**: TSC accumulato mentre un task gira.
- **idle**: TSC accumulato in `hlt`.

Il rapporto `busy / (busy + idle)` è la CPU% mostrata da `rtop`. `tsc_per_ms`
permette la conversione in millisecondi.

I contatori sono esposti dalle host fn `cpustat` (wasmi) e `sys.cpustat`
(Wasmtime) — stessa struttura binaria.

## Contratti

- **Solo il BSP tocca I/O**: VFS, runtime WASM, device driver, executor. Gli AP
  eseguono solo job puri compute.
- **I job SMP sono puri**: `fn(&[u8]) -> u64`, nessun riferimento a stato mutabile
  globale, nessun lock. Violarlo è undefined behavior (data race).
- **Un IPI sveglia gli AP**: `smp::pool::wake_aps()` invia un IPI broadcast. Gli
  AP rispondono prendendo job dalla coda.
- **Il timer IRQ (100 Hz) è l'unica fonte di wake** per l'executor: senza di esso
  i task sleeping non si svegliano.

## Vincoli e limiti

- **Nessun preemptive scheduler**: un task che non fa `.await` (o che non esaurisce
  il fuel) occupa il BSP per la durata del suo slice. Niente time-sharing.
- **No multi-queue executor**: c'è un solo executor, sul BSP. I task WASM non
  possono migrare su un AP.
- **Job pool a slot fissi**: il numero di job in volo è limitato (cap nel pool).
  Un submit quando il pool è pieno fallisce e il BSP esegue inline.
- **No NUMA awareness**: tutti i core vedono la stessa memoria flat.

## Insidie / note

- Il supervisor **6-detect** (`smp/`) monitora se il GUI core risponde (heartbeat).
  Se il compositor smette di battere il cuore, il supervisor lo segnala — ma non
  lo uccide (non c'è recovery).
- `asm!("cld")` prima di ogni invocazione Wasmtime: il codice Cranelift usa
  `rep movs` che gira all'indietro con DF=1, corrompendo i dati. La SysV ABI
  richiede DF=0.
- L'AP ha bisogno di **SSE/SIMD abilitato**: senza `CR0.EM=0, CR4.OSFXSR=1` il
  codice Cranelift trapla su istruzioni SIMD. Fix applicato nel trampoline AP.

## Vedi anche

- [Boot a fasi](boot-phases.md) — fase 3 (SMP bring-up)
- [Compositor](compositor.md) — compositing parallelo a bande
- [Runtime WASM](wasm-runtime.md) — fiber e fuel
- [Architettura — panoramica](../architecture/overview.md)
- [Indice della wiki](../README.md)
