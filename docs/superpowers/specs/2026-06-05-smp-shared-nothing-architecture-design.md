# Design — SMP shared-nothing / thread-per-core per ruos

**Data:** 2026-06-05
**Stato:** design / north-star (nessun codice ancora). Discussione architetturale.
**Tesi:** rendere ruos **prestante e solido sui 4–8+ core** restando fedele al
pivot (sandbox = WASM, niente preemption, niente ring 3, single address space).
La strada è **thread-per-core / shared-nothing**, NON uno scheduler preemptive
con lock ovunque.

---

## 0. TL;DR

- **Oggi:** UN executor cooperativo gira sul solo **BSP**; gli AP sono parcheggiati
  in `hlt` e fanno solo da **compute-pool** (job puri `fn(&[u8])->u64`). Tutto
  l'I/O (rete, USB, storage, shell, SSH) e la GUI girano sullo stesso core. Un
  task che non cede (il compositor fa busy-spin) **blocca tutto** — per questo
  SSH muore quando parte la GUI.
- **Target:** **un executor cooperativo per core**; ogni core **possiede** una
  fetta di stato; i core comunicano per **messaggi** (code lock-free + IPI), non
  per lock condivisi. È il modello Seastar/ScyllaDB, Erlang (scheduler-per-core),
  Redis-sharded. Il sandbox WASM **è già** shared-nothing → mappa naturalmente.
- **Solidità senza preemption:** confinamento (un core ostile freeza solo sé) +
  **supervisione** (heartbeat per-core + restart) + fuel/deadline.
- **Anti-pattern da evitare:** preemption + `IrqMutex` su ogni globale =
  "Linux fatto peggio" → contesa + deadlock = fragile e lento, e tradisce il pivot.

---

## 1. Modello attuale (dal codice)

### 1.1 Executor singolo, cooperativo, sul BSP

`kernel/src/executor/mod.rs`:

- `run()` (`:51`) gira **solo sul BSP**, spawna i task e entra nel poll-loop
  (`:78`): azzera `WAKE_PENDING` (`:24`), `exec.poll()` (`:85`), poi `sti;hlt`
  atomico (`:102`) se niente è pronto. Il timer IRQ (100 Hz) risveglia.
- Asserzione esplicita single-core (`:32–42`, verbatim):
  > "The cooperative executor is single-core BY DESIGN (the 2026-05-28 pivot…),
  > no preemptive scheduler, no SMP run-queue. `crate::cpu::PER_CPU`/`this_cpu()`
  > provide per-core data for a FUTURE SMP phase, but the run-queue here is NOT
  > yet SMP-safe".
- **Yield primitive:** `kernel/src/executor/delay.rs` — `Delay::ticks(n).await`
  (`:64`), lista a 64 slot drenata dal timer ISR (`timer_tick`, `:151`), guardia
  ABA via `GEN_COUNTER`. È così che i task cedono (es. `net_poll_task` →
  `Delay::ticks(1).await` = 10 ms).
- **Task spawnati** (`:62–75`): `tick_task`, `net_poll_task`, `usb_poll_task`,
  `console_drain_task`, `boot_shell_task`, `exec_worker_task`,
  `pipeline_worker_task`, `ssh_serve_task`, `ssh_pty_dispatcher_task`,
  `pty_watchdog_task`, `service_dispatcher_task`.

### 1.2 SMP oggi = solo compute-pool offload (AP parcheggiati)

- Bring-up: `kernel/src/smp/mod.rs::bringup()` (`:8`) avvia gli AP via Limine MP,
  assegna `cpu_id` densi, attende `mark_online()`.
- AP: `kernel/src/cpu/ap.rs::ap_entry` (`:17`) carica GDT/TSS per-core + IDT
  condivisa, init LAPIC, poi `ap_worker_loop` (`:35`): drena job dal pool
  (`pool::take`), li esegue, `enable_and_hlt` se vuoto; svegliato da IPI `VEC_WAKE`.
- Pool: `kernel/src/smp/pool.rs` — 64 slot, `JobFn = fn(&[u8]) -> u64` (`:20`,
  **puro, no captures, no I/O**), coda `IrqMutex<VecDeque<usize>>` (`:50`),
  `submit`/`take`/`run_slot`/`poll_done`. **Non gira task async, solo funzioni pure.**
- Identità core: `kernel/src/cpu/mod.rs` — `cpu_id()` (`:127`) via tabella
  `LAPIC_TO_CPU` (NO `gs:[0]`: quirk VirtualBox documentato `:64–68`), `PER_CPU`
  array per-core (`:50`), GDT/TSS/IDT per-core indicizzati.

### 1.3 Come è guidato l'I/O (oggi, tutto sul BSP)

| Sottosistema | Driver | Modello | Cadenza | Stato toccato |
|---|---|---|---|---|
| Rete (smoltcp + virtio-net/e1000) | `net_poll_task` → `net::poll()` (`net/mod.rs:96`) | **polled** (no MSI) | `Delay::ticks(1)` = 10 ms | `NET: Mutex<Option<NetState>>` (`net/mod.rs:36`) |
| USB (xHCI + HID) | `usb_poll_task` → `usb::poll()` (`usb/mod.rs:28`) | **polled** (no MSI) | 10 ms | `SLOTS`/`WORK` `IrqMutex` (`usb/registry.rs:36,43`) |
| Tastiera PS/2 | ISR IRQ1 `VEC_KEYBOARD=0x21` (`keyboard/mod.rs:122`) | **IRQ** | per evento | PTY 0 master, o gfx queue in GUI |
| Mouse PS/2 | ISR IRQ12 `VEC_MOUSE=0x22` (`mouse/mod.rs:97`) | **IRQ** | per evento | `QUEUE: IrqMutex<VecDeque<MouseEvent>>` |
| Timer | ISR LAPIC `VEC=0x20` (`timer.rs:11`) | **IRQ** periodico | 100 Hz | `TICKS`, sveglia `Delay` |
| Storage AHCI/SATA | qualunque fiber via VFS (`ahci/port.rs:246`) | **polled sincrono** (busy-loop su `ticks`) | on-demand | `PORT0` `Mutex<Option<AhciPort>>` |
| Console | `console_drain_task` | async coop | await su PTY0 out | `CONSOLE`/`LOG` mutex |
| Shell locale / SSH | `boot_shell_task`, `ssh_serve_task`, `ssh_pty_dispatcher_task` | async coop | await | PTY pairs, TCP socket |
| WASM exec | `exec_worker_task` (`:113`) | async coop / fiber | await su `EXEC_QUEUE` | istanze wasmi/wasmtime |
| Compositing GUI | `dispatch_bands` → `pool::submit` (`wm.rs:210`) | **SMP parallelo** (già!) | per-frame | back-buffer a bande disgiunte |

### 1.4 Il difetto attuale: la GUI blocca l'executor

- `exec_worker_task` (`executor/mod.rs:113`), sul ramo `compositor.cwasm`
  (`:132`), chiama `run_compositor_gate` (`wm.rs:1217`) che **non ritorna mai**.
  Commento verbatim (`executor/mod.rs:129–131`):
  > "It owns the CPU and never returns — like the single-GUI path — so the exec
  > task blocks here. That is intentional for the visual gate."
- `Compositor::run()` (`wm.rs:1096`) è un loop `-> !` che fa
  reap → fold_mouse → frame_all → present → **busy-spin di pacing**
  (`:1210`: `for _ in 0..2_000_000 { spin_loop() }`).
- Conseguenza: `exec.poll()` non ritorna mai → `net_poll_task`/`ssh_*`/`usb_poll_task`
  **non vengono mai più pollati** → rete/SSH morti. PS/2 (IRQ) sopravvive; USB è
  pompato a mano in `fold_mouse` (`gfx/mod.rs:235`).
- **Già parallelo:** il compositing è l'unico carico che usa gli AP
  (`dispatch_bands`/`composite_band_job`, `wm.rs:169,210` + `compose.rs`): bande
  a righe disgiunte → nessun lock. È il **seme** del modello target.

---

## 2. Inventario stato condiviso (cosa serve un owner)

Classificazione per la migrazione shared-nothing.

### 2.1 Globale VERO (single address space — resta condiviso)
- **Memoria:** `ALLOCATOR` talc (`memory/heap.rs:19`, lock su ogni alloc),
  `MAPPER`/`HHDM_OFFSET` (`memory/mapper.rs:13`), `NEXT` W^X exec (`memory/exec.rs:16`).
- **MMIO/CPU:** `LAPIC_VIRT` (`apic/lapic.rs:31`), `IOAPIC_VIRT` (`apic/ioapic.rs:10`),
  `IDT` (`idt.rs:17`).
- Strategia: lock-free o spinlock SMP **microscopico**, mai attraverso await/messaggio.
  L'allocatore è il candidato #1 a **arene per-core** (vedi §3.3).

### 2.2 Read-mostly (config/init — RCU o immutabile dopo init)
- Geometria framebuffer `GFX_*` (`gfx/mod.rs:19`), calibrazione clock
  `BOOT_TSC`/`TSC_PER_MS` (`boot/clock.rs:14`), tabelle scancode
  (`keyboard/mod.rs:17`), engine Wasmtime `ENGINE` (`wasm/wt/mod.rs:147`), ACPI,
  snapshot HBA, host key SSH `SESSION_CTX` (`ssh/server.rs:16`), `LAPIC_TO_CPU`.
- Strategia: pubblica-una-volta + lettura libera (nessun lock nel path caldo).

### 2.3 Read-write hot (il vero rischio di contesa — da dare un owner)
- `EXECUTOR` (`executor/mod.rs:45`) → **uno per core** (§3.1).
- `NET` (`net/mod.rs:36`) → **owner: un net-core** (§3.4).
- `CONSOLE`/`LOG` (`console/mod.rs:67`, `klog.rs:56`) → **owner + canale log**.
- `ALLOCATOR` (`memory/heap.rs:19`) → **arene per-core** + fallback globale.
- `REGISTRY`/`NEXT_PID` proc (`proc.rs:25`) → tabella RCU o owner + query async.
- Code IRQ-safe (già `IrqMutex`, basse-contesa): mouse `QUEUE` (`mouse/mod.rs:56`),
  gfx `EVENTS` (`gfx/mod.rs:191`), USB `SLOTS`/`WORK`, pool `QUEUE`.
- PTY `PAIRS[4]` (`pty/mod.rs:12`) → owner per-pair o per-core.
- RNG `RNG` (`rng.rs:8`) → per-core (ChaCha re-seedabile) o owner.

### 2.4 Già partizionato per-core (modello da imitare)
- `PER_CPU[]` (`cpu/mod.rs:50`), GDT/TSS/`DOUBLE_FAULT_STACK` per-core
  (`gdt.rs:21,27,31`), `sched/cpustat CORE[16]` (`sched/cpustat.rs:27`).
- Questo è **esattamente** il pattern target esteso a tutti i sottosistemi.

---

## 3. Architettura target: thread-per-core / shared-nothing

> Un executor cooperativo per core. Ogni core possiede una fetta di stato. I core
> comunicano per messaggi, non per lock. Cooperativo DENTRO il core, parallelo TRA
> i core.

### 3.1 Executor per-core
- Ogni core esegue il proprio `RawExecutor` + run-queue + `Delay` list + idle/hlt.
  Il commento in `executor/mod.rs:34–42` lo prevede già ("a FUTURE SMP phase").
- Boot: gli AP non parcheggiano più nel solo `ap_worker_loop` (compute-pool); ognuno
  entra in `executor::run()` con la propria istanza. `EXECUTOR` diventa
  `PER_CPU`-locale, non un singleton.
- `WAKE_PENDING` → per-core; il `__pender` (`:437`) sveglia il core proprietario
  del task (IPI mirato se il task vive su un altro core).
- **Costo:** la run-queue e i waker devono diventare SMP-safe nel solo punto di
  cross-core enqueue (spostare un task tra core), tutto il resto resta locale.

### 3.2 Ownership dello stato — le 3 regole
1. **Affinity:** ogni sottosistema vive su UN core. Accesso da fuori = **messaggio**,
   mai un lock. Es. lo stack rete (`NET`) è di proprietà del *net-core*; un fiber su
   un altro core posta una richiesta socket e await la risposta.
2. **Per-core replica:** allocatore (arene), RNG, stat, contatori → copie per-core →
   zero contesa.
3. **RCU/immutabile** per read-mostly (§2.2): publish-then-read, lettori mai bloccati.

I pochi globali veri (§2.1) restano dietro spinlock SMP tenuti microscopicamente.

### 3.3 Allocatore — il primo collo di bottiglia
- `ALLOCATOR` talc è lockato su **ogni** alloc/free. Con N executor → contesa
  immediata. Soluzione standard: **arene heap per-core** (slab/magazine locali) con
  fallback al pool globale per i blocchi grandi. È prerequisito di TUTTO il resto
  (ogni task alloca).

### 3.4 Message bus inter-core
- Code lock-free **SPSC/MPSC** (una per coppia di core o per-core inbox) + IPI come
  wake. Generalizza ciò che `smp/pool.rs` fa già per i job puri, ma trasporta
  **richieste async** (con waker del mittente) non solo `fn(&[u8])->u64`.
- API tipo: `core_send(target, Msg)` → l'inbox del target; il task mittente await
  un `oneshot` risvegliato quando il target risponde. Nessuno stato condiviso
  mutabile attraversa il confine: solo messaggi (request/response).

### 3.5 Pin dei workload (mappa core → ruolo)
- **I/O core(s):** `net_poll_task`, `usb_poll_task`, AHCI, `ssh_*`, PTY, console.
- **GUI core(s):** compositor/egui — il loop `run()` gira QUI, fuori dal core I/O.
  Anche cedendo per-frame, non compete più con SSH. Il render pesante si fa
  fan-out sugli AP liberi (estende `dispatch_bands` al raster egui, non solo al
  composite).
- **App-WASM core(s):** ogni istanza WASM è di un core (linear memory + fuel +
  store). Lo scheduling di una nuova app = assegnala al core meno carico. **WASM
  non condivide memoria** → fit perfetto con shared-nothing.

### 3.6 Solidità — confinamento + supervisione (no preemption)
- **Confinamento:** un task che non cede freeza **il suo core**, non la macchina.
- **Supervisore** (su un core diverso): heartbeat per-core; se un core è muto →
  killa l'istanza WASM colpevole / resetta quell'executor. Albero di supervisione
  stile Erlang applicato ai core.
- **Fuel/deadline:** il fuel-metering WASM (`wasm/`) già uccide le app che non
  cedono; estendere un deadline-check ai job kernel lunghi (cedi-o-muori).
- **Panic per-core:** il panic handler (`main.rs`) diventa per-core + restart,
  così un core in panic non porta giù il sistema.

---

## 4. Piano di migrazione (ordine di leva)

Ogni passo è ancorato al codice attuale. **Niente big-bang**: incrementale.

1. **Allocatore per-core** (`memory/heap.rs`). Prerequisito: senza arene locali,
   N executor si scannano sul lock talc. Slab per-core + fallback globale.
2. **Message bus inter-core** (nuovo, generalizza `smp/pool.rs`). Code lock-free +
   IPI + oneshot reply. È l'infrastruttura su cui poggia tutto.
3. **Executor per-core** (`executor/mod.rs`, `cpu/ap.rs`). Gli AP entrano in
   `run()` con executor locale invece del solo `ap_worker_loop`. `EXECUTOR` →
   per-`PER_CPU`. Cross-core enqueue via §3.4.
4. **Audit ownership dello stato** (§2): per ogni globale RW-hot decidi owner /
   per-core / RCU. È il lavoro vero. Inizia da `NET` (un net-core) e `CONSOLE/LOG`
   (un log-core + canale).
5. **Pin dei workload** (§3.5): sposta il compositor/GUI su un core dedicato
   (risolve subito SSH-durante-GUI, §1.4, senza neppure rendere la GUI cooperativa —
   gira parallela). Generalizza il fan-out del raster sugli AP.
6. **Supervisore + panic per-core** (§3.6): heartbeat, restart, deadline kernel.
7. **SMP-safety dei globali veri** (§2.1): spinlock SMP microscopici dove resta
   condivisione (allocatore fallback, paging).

**Quick win indipendente (oggi):** rendere `Compositor::run()` cooperativo
(`Delay::ticks(1).await` al posto del busy-spin `wm.rs:1210` + `run_compositor_gate`
async + `.await` in `executor/mod.rs:133`) ripristina SSH/rete **subito** sul
modello attuale single-core. È il ponte verso il punto 5 (poi la GUI passa su core
suo). NON sostituisce la migrazione, ma toglie il sintomo.

---

## 5. Trade-off onesti / limiti

- **Non elimina del tutto l'ingordo:** un task che busy-loopa freeza il suo core
  (gli altri vivono). Confinamento, non immunità. La preemption vera darebbe
  immunità; il prezzo (ring 3, context-switch, scheduler) è fuori dal pivot.
- **Latenza dei messaggi:** ciò che era una chiamata locale (es. una `read` VFS)
  diventa un round-trip inter-core quando l'owner è altrove. Mitigazione: affinity
  (metti chi usa X sullo stesso core di X) + batch.
- **Disciplina:** ogni nuovo sottosistema deve dichiarare il suo owner e cedere.
  La regressione SSH-in-GUI è la prova di cosa succede a violarla.
- **Coerenza memoria:** arene per-core richiedono cura nel free cross-core (un
  blocco allocato su core A e liberato su core B). Pattern noti (magazine,
  remote-free queue).

## 6. Cosa NON fare (anti-pattern)

- **NO scheduler preemptive + ring 3 + per-process page tables.** Tradisce il pivot
  (sandbox = WASM), aggiunge SYSCALL/SYSRET, TSS RSP0, context-switch. Esplicitamente
  droppato (vedi `roadmap-rust-os.md`, CLAUDE.md).
- **NO `IrqMutex` su ogni globale "per renderlo SMP-safe".** Contesa + deadlock =
  fragile e lento = "Linux fatto peggio". L'ownership+messaggi elimina le race
  *by construction*, i lock le spostano solo a runtime.

## 7. Perché è coerente con la tesi di ruos

`sandbox = WASM (non ring 3)` e `parallelismo = core shared-nothing (non
preemption+lock)` sono **la stessa filosofia: isolamento per ownership**. WASM
isola le app per linear-memory; shared-nothing isola i core per stato posseduto.
Un'istanza WASM **è già** una unità shared-nothing. Estendere il modello al kernel
non è un innesto estraneo: è la **conseguenza naturale** del pivot. Il risultato
è un OS distintivo — WASM-first, shared-nothing, cooperativo-per-core — non "un
altro Unix".

---

## Riferimenti codice (ancore)

- Executor single-core + asserzione: `kernel/src/executor/mod.rs:32,45,51,62,113,129`
- Yield/Delay: `kernel/src/executor/delay.rs:64,151`
- SMP bring-up + AP loop: `kernel/src/smp/mod.rs:8`, `kernel/src/cpu/ap.rs:17,35`
- Compute pool: `kernel/src/smp/pool.rs:20,50,54`
- Per-CPU: `kernel/src/cpu/mod.rs:50,127`; GDT/TSS per-core: `kernel/src/gdt.rs:21,27,31`
- Sync: `kernel/src/sync/mod.rs:14`
- Compositor che blocca: `kernel/src/wasm/wt/wm.rs:1096,1210,1217`; banded:
  `wm.rs:169,210`, `kernel/src/wasm/wt/compose.rs`
- Globali per subsystem: vedi §2.
