# Design — SMP shared-nothing: spec macro della migrazione (foundation + 7 step)

**Data:** 2026-06-05
**Stato:** design / spec macro. Nessun codice ancora. Copre l'intero programma §4 del
north-star doc come UN'unica spec di architettura; ogni step avrà poi il suo
ciclo *spec di dettaglio → piano → implementazione*.
**Tesi:** rendere ruos **prestante** sui 4–8+ core restando fedele al pivot
(sandbox = WASM, niente preemption, niente ring 3, single address space). Strada =
**thread-per-core / shared-nothing massimale** (un executor cooperativo per core,
ogni core possiede una fetta di stato, i core comunicano per messaggi), NON uno
scheduler preemptive con lock ovunque.

**Documento padre (discussione/north-star):**
`docs/superpowers/specs/2026-06-05-smp-shared-nothing-architecture-design.md`.
Questa spec lo rende **azionabile**: corregge due assunzioni sbagliate, fissa
l'ordine di build, e promuove a lavoro di prima classe sei gap di correttezza che il
north-star non copriva.

---

## 0. TL;DR — cosa cambia rispetto al north-star doc

Un'indagine fondata sul codice (8 sottosistemi + critico di consistenza) ha
**corretto** due premesse e **scoperto** il vero gap foundational; uno spike
successivo ha **riformulato Step 1** (punto 4).

1. **I lock attuali sono già spinlock SMP veri, non stub single-core.** `spin 0.9.8`
   (`Cargo.lock`) è un lock CAS cross-core; `IrqMutex` (`sync/mod.rs:14-72`) =
   `spin::Mutex` + IF-mask. `CHANGELOG/186` ha auditato ~52 siti con **zero
   must-fix**. → **Non esiste uno "step-0: costruisci uno spinlock".** La migrazione è
   **riduzione contesa + partizione ownership** su una base di lock già sana.

2. **Gli AP sono già vivi multi-core OGGI.** `boot/phases/interrupts.rs:52` →
   `smp::bringup()` → `smp/mod.rs:50` `cpu.bootstrap(ap_entry, id)` → ogni AP entra in
   `cpu/ap.rs:35 ap_worker_loop` e drena `smp::pool` concorrentemente. Il marker
   `composite cores={N}` (`wm.rs:1205`) lo prova. "Parcheggiati in hlt" = idle nel
   loop, non "mai avviati". Il path spinlock SMP è **esercitato adesso**, non dormiente.

3. **Il vero gap foundational** non è lo spinlock: è il **dispatch wake/`__pender`
   cross-core** (consegnare/svegliare un waker da core A a core B). Oggi `__pender`
   è globale (un solo `WAKE_PENDING`, `executor/mod.rs:24,437`). Step 2/3/4 ne hanno
   bisogno tutti e tre → va progettato **una sola volta** (vedi §3).

4. **Step 1 riformulato dallo spike (2026-06-05).** Un allocatore per-core REGREDISCE
   finché `cpu_id()` costa ~200 ns (LAPIC MMIO) — **misurato**, non ipotesi. Step 1 si
   divide: **1a = `cpu_id()` veloce via `RDTSCP`+`IA32_TSC_AUX`** (verificato portabile
   su QEMU/VirtualBox/HW reale; RDPID scartato perché non portabile), poi **1b =
   allocatore per-core ri-misurato**. Dettaglio §6.

> **Conseguenza di framing:** la domanda non è "i lock reggono l'SMP?" (sì). È:
> "come partizioniamo l'ownership così che i core NON debbano contendersi gli stessi
> lock sul path caldo, e come svegliamo un task che vive su un altro core?"

---

## 1. Modello target — CORE_ROLES (ownership distribuita massimale)

Un executor cooperativo per core. Ogni core **possiede** una fetta di stato. Accesso
da fuori = **messaggio** (bus inter-core + IPI), mai un lock condiviso sul path caldo.
Cooperativo DENTRO il core, parallelo TRA i core.

Tabella ruoli (autorità unica, vedi Step 5 — `CORE_ROLES` single source of truth):

| Core | Ruolo | Possiede |
|---|---|---|
| **BSP (0)** | `BspIo` | NET (smoltcp), `usb_poll`, SSH, boot shell, exec dispatch, infra bus/wake |
| **GUI-core** | `GuiCompositor` | solo il compositor (loop spinner; **niente executor**) |
| **log-core** | `LogOwner` | CONSOLE + SERIAL + klog; `kprintln!` async via messaggio (+ fallback sync) |
| **pty-core(i)** | `PtyOwner` | coppie PTY; keyboard ISR / sessioni SSH postano master-input via bus |
| **compute/app core(i)** | `ComputeApp` | `smp::pool` + executor per-core con istanze WASM app |

Più (replicati per-core, non un core dedicato):
- **proc registry**: per-core — ogni executor traccia i PID che possiede; `ps`/kill
  cross-core via bus query.
- **RNG**: replica ChaCha20 per-core (seed da entropy bank BSP; nessun lock).

**Decisione di affinity (riconcilia conflitto 1 × conflitto 4):** ownership
**distribuita** in generale, MA **NET resta pinnato sul BSP** — il net-owner è il BSP,
perché net/usb/ssh si usano a vicenda e devono stare sullo stesso core (l'alternativa
sarebbe un round-trip cross-core per ogni operazione SSH↔socket). log-core e
pty-core sono owner **separati** quando il conteggio core lo consente.

**Degrado per conteggio core (la tabella è dinamica, assegnata a `bringup`):**
- **1 core**: tutto sul BSP (path di oggi). Compositor inline, USB/SSH affamati in
  GUI (limitazione nota, fallback).
- **2 core**: BSP = tutto I/O + tutti gli owner (NET+log+pty+proc) + dispatch;
  GUI-core = compositor. Le bande compositing girano inline sul GUI-core.
- **3 core**: + un `ComputeApp` core (pool + app WASM).
- **4+ core**: log-core e pty-core si separano dal BSP; restano `ComputeApp` core per
  pool/app. Con ≥6 si possono splittare più pty-core / più app-core.

---

## 2. Invarianti globali (ogni step DEVE preservarli)

Dal critico di consistenza. Sono i vincoli che tengono insieme il modello.

1. **Fedeltà al pivot.** Niente scheduler preemptive, niente ring 3, niente
   page-table per-processo, single address space. SMP = N executor **cooperativi** +
   messaggi, NON uno scheduler SMP preemptive.
2. **Single address space condiviso.** Un PML4, un `MAPPER` (`mapper.rs:13`), un
   HHDM offset. "Per-core" ≠ "per-address-space": shared-nothing si applica allo
   **stato dei task e all'ownership**, non alle page table (i globali veri restano
   condivisi — Step 7).
3. **Ordine lock.** `MAPPER.lock()` POI `FRAMES.lock()`, mai invertito
   (`frames.rs:144`, `mapper.rs:13`). Mai tenere un lock attraverso un `await` o
   attraverso un send/wait di messaggio cross-core.
4. **Baseline spinlock reale.** `spin::Mutex`/`IrqMutex` sono già SMP-corretti: la
   migrazione **non deve regredire** questo (es. sostituire uno spinlock corretto con
   una struttura lock-free che ha un bug di ordering). Obiettivo = ridurre contesa;
   invariante = preservare mutua esclusione.
5. **Stato per-core = single-writer-per-slot.** `PER_CPU[cpu_id]`, `GDT[cpu_id]`,
   `TSS[cpu_id]`, `DOUBLE_FAULT_STACK[cpu_id]`, `cpustat CORE[cpu_id]` e ogni nuovo
   array per-core (arena/executor/delay-list/heartbeat) si scrivono **solo** dal core
   proprietario (o una volta a boot prima che quel core parta). Letture cross-core OK;
   scritture cross-core rompono la partizione.
6. **Path ISR lock-light e IF-aware.** Stato condiviso con ISR usa `IrqMutex`
   (IF-mask + spinlock) o `try_lock`-con-defer (lato ISR di `delay.rs`). Nessuna ISR
   blocca su un lock tenuto da un task dello stesso core. Vale per ogni nuovo timer
   ISR per-core (Step 3) e handler IPI inbox (Step 2).
7. **Correttezza dei wake (no missed wake).** Il pattern atomico `sti;hlt`
   (`executor/mod.rs:102`, `cpu/ap.rs:50`) e "set-flag prima dell'IPI, check-flag con
   IF-disabled prima di hlt" vanno preservati **per-core**. Ogni `WAKE_PENDING` /
   inbox-pending flag per-core si setta PRIMA dell'IPI e si controlla sotto IF-disable.
8. **Clock monotonico globale.** `TICKS` (`timer.rs:9`) è il tempo monotonico
   boot-wide. Se gli AP ottengono il proprio timer ISR (Step 3), `TICKS` resta **un
   solo contatore globale** (o ridefinito) così i target `Delay` e i timestamp
   restano comparabili tra core. NON forkare il wall-clock per core.
9. **Fencing publish/consume per handoff cross-core.** Ogni handoff dati cross-core
   (mailbox ptr/len, inbox Box, migrazione arena per-core) usa **Release** sullo store
   finale "ready" del produttore e **Acquire** sulla prima lettura del consumatore, con
   l'IPI come wake. È LA regola che tiene sani i path lock-free.
10. **Invarianti hardware BSP-only finché non migrati esplicitamente.** Scritture
    `IOAPIC` redirect (`ioapic.rs:10`), init LAPIC/timer, routing tastiera PS/2 sono
    BSP-only oggi. Ogni step che lascia un AP toccarli (hot-add I/O, timer AP) deve
    aggiungere sincronizzazione PRIMA.

---

## 3. Foundation — wake/`__pender` cross-core (il vero step-0 nuovo)

**Problema.** Oggi `__pender` (`executor/mod.rs:437`) fa solo
`WAKE_PENDING.store(true)` — un singolo `AtomicBool` globale (`:24`). Funziona perché
c'è un solo executor sul BSP. Nel modello target un task su core A può essere
risvegliato da un evento su core B (reply a un messaggio, completamento di un job,
input instradato). Serve una primitiva di wake **per-core** che:

1. setti il `WAKE_PENDING` **del core proprietario** del task, e
2. se il chiamante è su un altro core, gli mandi un **IPI mirato** per farlo uscire da
   `hlt`.

**Design (unico, condiviso da Step 2/3/4).**
- `WAKE_PENDING` diventa per-core: `static WAKE_PENDING: [AtomicBool; MAX_CPUS]`
  (oppure campo in `PerCpu`).
- `__pender(context: *mut ())` usa il `context` per identificare il core proprietario.
  embassy passa il `context` registrato alla creazione dell'executor → vi codifichiamo
  il `cpu_id` proprietario. `__pender` allora:
  - `WAKE_PENDING[owner].store(true, SeqCst)`;
  - se `cpu_id() != owner` → `send_ipi(owner, VEC_WAKE)` (IPI mirato, non broadcast).
- **Waker Send/Sync cross-core** (gap di correttezza): un `Waker` embassy è legato a
  un `RawExecutor`. Va **verificato** che `.wake()` chiamato da un altro core sia
  safe. Il design lo rende safe *by construction*: `.wake()` cross-core NON tocca la
  run-queue dell'altro executor — esegue solo `__pender` (set flag + IPI). La
  ri-poll del task avviene **sul core proprietario** quando esce da `hlt`. Nessuna
  struttura dell'executor remoto viene mutata dal core chiamante. (Da confermare in
  implementazione con un test mirato — vedi §11 e Step 3.)

**Perché è foundational.** Step 2 (reply waker del bus), Step 3 (cross-core spawn),
Step 4 (inbox owner che sveglia il mittente) dipendono **tutti** da questa singola
primitiva. Se ogni step costruisce il proprio meccanismo di wake → tre path di wake
incompatibili. Va costruita una volta, in Step 2/3 congiunti.

---

## 4. Ordine di build (corretto dal critico)

```
Step 7 (audit/baseline globali)         ← PRIMO: è CHANGELOG/186 formalizzato, niente codice nuovo
   │   definisce i target di contesa su cui agiscono 1 e 6
   ▼
[verifica AP vivi]                      ← prerequisito di lettura, già confermato (§0.2)
   ▼
Step 1a (fast cpu_id: RDTSCP+TSC_AUX)   ← PREREQUISITO MISURATO: senza, l'indicizzazione
   │   per-core (alloc + tutto shared-nothing) paga ~200ns/accesso. Detect+fallback LAPIC.
   ▼
Step 1b (allocatore per-core)           ← DOPO 1a + ri-misura; se non vince, rinvia a Step 3
   │   prerequisito HARD di Step 3 (ogni task alloca)
   ▼
Step 2 (message bus + WAKE cross-core + tabella vettori IPI)
   │   dip. soft da 1 (payload Box; fallback globale)
   ▼
Step 3 (executor per-core + timer AP + TLB shootdown)
   │   dip. HARD da 1 e 2
   ▼
┌─────────────────────────┬──────────────────────────┐
Step 4 (ownership)         Step 5 (pin GUI + CORE_ROLES)   ← in PARALLELO (indipendenti tra loro)
   dip. da 2,3                dip. da 2,3
└─────────────────────────┴──────────────────────────┘
   ▼
Step 6 (supervisor + per-core panic)
   6-detect anticipabile (solo legge atomici); 6-recover dip. da 3,4
   RNG-per-core (sotto-item di Step 4) anticipabile (nessun bus)
```

> **Anticipabili:** Step 6-detect (heartbeat + supervisor che legge atomici) e
> RNG-per-core possono essere portati avanti in qualsiasi momento dopo §0.2.

Il grafo differisce dal §4 letterale del north-star per tre punti: **7 va per primo**
(baseline scritta), **Step 1 si divide in 1a (fast cpu_id) + 1b (allocatore)** dopo lo
spike (§6), e **5 va in parallelo a 4** (non dopo).

---

## 5. Step 7 — Audit/baseline dei globali veri (PRIMO)

**Obiettivo.** Formalizzare quali globali restano condivisi e sotto quale disciplina.
Niente codice nuovo: documentazione in-code + un changelog che promuove `CHANGELOG/186`
a baseline della migrazione. Definisce i target di contesa per Step 1/6.

**Stato attuale (classificato).**
- **Globali veri (restano condivisi, spinlock microscopico):** `ALLOCATOR`
  (`heap.rs:19`), `FRAMES` (`frames.rs:144`), `MAPPER`+`HHDM_OFFSET` (`mapper.rs:13-14`),
  `SERIAL` (`serial.rs:31`), `CONSOLE` (`console/mod.rs:67`).
- **Init-once / read-only dopo init:** `LAPIC_VIRT` (`lapic.rs:31`, semantica per-CPU,
  stesso virt su ogni core), `IOAPIC_VIRT` (`ioapic.rs:10`, BSP-only writer), `IDT`
  (`idt.rs:17`, reentrant).
- **Atomici con ordering corretto:** `GFX_*` geometry (`gfx/mod.rs:14-24`,
  Release/Acquire), `TICKS` (`timer.rs:9`, Relaxed, BSP-only oggi), `WAKE_PENDING`
  (`executor/mod.rs:24`), `LAPIC_TO_CPU`/`CPUS_ONLINE` (`cpu/mod.rs`).
- **Per-core partizionato (single-writer-per-slot):** `PER_CPU`, `GDT`/`TSS`/
  `DOUBLE_FAULT_STACK` (`gdt.rs:21-31`), `cpustat CORE` (`cpustat.rs:14-27`).

**Target/deliverable.**
- Commento doc su ogni globale: classe (spinlock / init-once / atomico) + invariante.
- Stato esplicito dell'**ordine lock** `MAPPER→FRAMES` in codice (commento + eventuale
  macro debug `assert_lock_order` sotto `boot-checks`).
- `FIXME` linkati a step futuri dove un AP toccherà un globale BSP-only (IOAPIC write
  su hot-add I/O; timer AP).
- Verifica memory-ordering: ogni load data-dependent di `GFX_*` accoppiato a una
  Acquire sull'atomico "notification" (`GFX_VIRT`).

**Touch points.** `heap.rs:19`, `frames.rs:144`, `mapper.rs:13-14`, `lapic.rs:31`,
`ioapic.rs:10`, `idt.rs:17`, `serial.rs:31`, `console/mod.rs:67`, `gfx/mod.rs:14-24`,
`timer.rs:9`, `executor/mod.rs:24`, `cpu/mod.rs`, `gdt.rs:21-31` — tutti **solo
commenti/doc** + eventuale macro debug.

**Test/accettazione.** Boot invariato (4 suite verdi). `boot-checks`: assert
`this_cpu().cpu_id == 0` a boot BSP; assert ordine lock dove esercitabile. Zero
regressioni.

**Rischi.** Nessuno funzionale (doc-only). L'unico rischio è "documentare e basta"
senza agire sulla contesa — ma quello è compito di Step 1/6, non di 7.

---

## 6. Step 1 — Fast cpu_id, poi allocatore per-core (RIFORMULATO)

> **Riformulazione (2026-06-05, post-spike).** Lo spike "allocatore per-core" (piano
> `docs/superpowers/plans/2026-06-05-smp-step1-allocator-spike.md`, decisione
> `docs/superpowers/decisions/2026-06-05-allocator-architecture.md`) ha **provato con
> i dati** che un allocatore per-core REGREDISCE finché `cpu_id()` costa ~200 ns. La
> causa non è la struttura dell'allocatore: è che `cpu_id()` legge LAPIC MMIO ed è
> chiamato su OGNI alloc. → **Step 1 si divide: prima un `cpu_id()` veloce (1a),
> poi l'allocatore per-core (1b), ri-misurato.**

### Risultato spike (misurato, `-smp 4`, QEMU)

| variant | cpuid ns/call | multi per_job (3 AP) |
|---|---|---|
| default talc | ~200 | **19.7 ms** ✅ |
| magazine (A) | ~200 | 31.9 ms (+62%) |
| per-core talc (B) | ~200 | 37.8 ms (+92%) |

Nel multi bench **ogni core alloca sul suo stato per-core → zero contesa allocatore
per A/B**; eppure il default vince. Quindi **tassa cpu_id (~400 ns/ciclo alloc+free) >
contesa talc a 3 core**. Togli la tassa e A/B dovrebbero vincere (il loro unico
svantaggio è quella tassa). I numeri single-core QEMU sono troppo rumorosi per
distinguere; ci si fida del multi.

### Step 1a — Fast `cpu_id()` (PREREQUISITO; risolve il costo cpu_id di §6/§14)

**Obiettivo.** Rendere `cpu_id()` economico (~pochi cicli, niente MMIO) così che
l'indicizzazione per-core (allocatore, e ogni accesso per-core del modello
shared-nothing) non paghi una tassa. **Diverse-system-safe: feature-detect + fallback,
mai assumere l'istruzione.**

**Soluzione (verificata DIRETTAMENTE su 3 ambienti, 2026-06-05).** `RDTSCP` +
`IA32_TSC_AUX` (MSR `0xC000_0103`): al bring-up ogni core fa `wrmsr TSC_AUX = dense
cpu_id`; poi `cpu_id()` = una `rdtscp` (ritorna TSC_AUX in ECX). `RDPID` sarebbe più
diretto ma **NON è portabile**.

| Ambiente | RDTSCP | RDPID | wrmsr TSC_AUX + readback |
|---|---|---|---|
| QEMU `-cpu max` | ✅ | ✅ | ✅ |
| **VirtualBox guest** | ✅ | ❌ | ✅ |
| HW reale (i7-8700 Coffee Lake) | ✅ | ❌ | ✅ |

`RDTSCP` + `TSC_AUX` funziona **ovunque**, incluso VirtualBox — l'ambiente dove
gs-base è rotto (`cpu/mod.rs` quirk VBox), che era l'alternativa scartata. RDPID è
mascherato da VBox e assente su Coffee Lake → non affidabile.

**Design.**
- `detect_rdtscp()` (BSP, una volta, post-lapic): `CPUID.80000001h:EDX[27]` → setta
  `RDTSCP_OK: AtomicBool`. Niente RDTSCP → resta `false` → fallback LAPIC (nessuna
  regressione su CPU esotiche/vecchie).
- `set_tsc_aux(dense_id)`: `wrmsr IA32_TSC_AUX=dense_id` (no-op se `!RDTSCP_OK`). BSP
  nel suo init (id 0); ogni AP come **PRIMA** istruzione di `ap_entry` (prima di
  qualunque `cpu_id()` fast-path), così nessun AP osserva un TSC_AUX stale.
- `cpu_id()`: se `RDTSCP_OK` → `rdtscp` (ECX = dense id, clobber EAX/EDX/ECX),
  altrimenti il path LAPIC esistente verbatim.

**Ordine/invariante.** `RDTSCP_OK` è globale: ogni core DEVE scrivere il suo TSC_AUX
prima di chiamare `cpu_id()` sul fast path. Per il BSP non c'è finestra (prima del
detect, il fallback LAPIC ritorna 0 = ciò che TSC_AUX=0 darebbe). Per gli AP, la
WRMSR è la prima cosa in `ap_entry` (gdt/idt/lapic/mark_online non chiamano
`cpu::cpu_id()` — `gdt::init` riceve l'id come argomento).

**Touch points.** `cpu/mod.rs` (`detect_rdtscp`/`set_tsc_aux`/`RDTSCP_OK` + fast path
in `cpu_id()`), `cpu/ap.rs` (`set_tsc_aux` prima riga di `ap_entry`), `init_bsp`
(detect + `set_tsc_aux(0)`). Diagnostico `cpu::probe_fast_cpuid()` (marker
`cpuprobe rdtscp=.. rdpid=.. tscaux_rw=..`) per validare ogni ambiente a boot.

**Test/accettazione.** `make test-boot` invariato (cpu_id corretto in tutto il boot +
SMP). Re-run `allocbench`: `cpuid ns_per_call` deve crollare da ~200 a cifra singola;
poi ri-misura A/B vs talc a `-smp 4` → verifica che con la tassa rimossa i per-core
non regrediscano più (e idealmente vincano sotto contesa). Fallback: su una CPU senza
RDTSCP, boot identico a oggi (path LAPIC).

### Step 1b — Allocatore per-core = MAGAZINE A (verificato dopo 1a)

> **RISOLTO (2026-06-05).** Ri-misurato lo spike CON `cpu_id()` veloce (23 ns, Step 1a):
> a `-smp 4` **entrambi i per-core battono il default talc, e magazine A vince netto.**
>
> | variant | per_job PRIMA (cpu_id ~200ns) | per_job DOPO (cpu_id 23ns) |
> |---|---|---|
> | default talc | 19.7 ms | 14.9 ms |
> | **magazine A** | 31.9 ms | **9.6 ms (−36% vs talc)** |
> | per-core talc B | 37.8 ms | 12.2 ms (−18%) |
>
> **Decisione: 1b = magazine A.** Evita del tutto l'allocatore su cache-hit; niente
> remote-free/align=16, niente `owner_of` O(16)/`drain_remote` per alloc (gli overhead
> di B). La versione di produzione tiene il canonical-class-layout + bypass align>16 già
> nel prototipo; profila la size-class table. Dettaglio:
> `docs/superpowers/decisions/2026-06-05-allocator-architecture.md` §8.
>
> NB: il caso free-cross-core che §13 attribuiva a "Step 1 remote-free" **non serve** col
> magazine — il talc globale possiede l'intero heap, quindi qualunque blocco vi rientra
> validamente; la cache per-core ricicla sul core che libera.

**Obiettivo (1b).** Togliere la contesa sul lock `ALLOCATOR` globale: con N executor
per-core, ogni alloc/free si serializzerebbe su uno spinlock. Arene heap per-core +
fallback globale per blocchi grandi. **Prerequisito HARD di Step 3.**

**Stato attuale.**
- `ALLOCATOR = Talck<spin::Mutex<()>, ErrOnOom>` (`heap.rs:19`). Una sola istanza talc
  globale; ogni alloc/free prende il lock su tutti i core.
- Heap 128 MiB, init-once da Limine memmap+HHDM (`heap.rs:14-16,57-90`).
- talc è single-arena by design (`Cargo.lock` talc 4) → niente supporto per-core
  nativo; le arene vanno esterne.
- `cpu_id()` via LAPIC reg + `LAPIC_TO_CPU` (`cpu/mod.rs:127-136`), ~200 cicli.

**Target.**
- Arena per-core indicizzata da `cpu_id()`: `ARENAS[cpu_id]` (slab/magazine).
- **Small alloc (< soglia, es. 2 MiB):** fast path locale, nessun lock condiviso (il
  core possiede la sua arena).
- **Large alloc (≥ soglia):** fallback al pool globale (`spin::Mutex`), preso di rado.
- **Free cross-core** (alloc su core A, free su core B): **remote-free queue** per-core
  (`IrqMutex<VecDeque<(ptr,size)>>`) drenata dal core proprietario su confini
  alloc/free. È un requisito di **correttezza**, non ottimizzazione (vedi §10 — gli
  `InboxMsg` di Step 2 sono Box che attraversano i core e vengono droppati dal
  ricevente: è esattamente il caso free-cross-core).

**Costo `cpu_id()` sul path caldo — RISOLTO da Step 1a (trade-off §14).** ~200 ns per
LAPIC-read a OGNI alloc rendono l'arena per-core più lenta dello spinlock globale (lo
spike lo ha **misurato**, non più ipotesi). La mitigazione (a) "gs-base" è scartata
(rotto su VBox). La soluzione adottata è **Step 1a: `cpu_id()` veloce via RDTSCP+
TSC_AUX** (verificato portabile incl. VBox). 1b si progetta/misura **sopra** un
`cpu_id()` già economico.

**Nuove API (firme, non codice).**
```
pub const LARGE_BLOCK_THRESHOLD: usize = 0x200000;        // 2 MiB
pub struct PerCpuArena { slab: Slab, remote_free: IrqMutex<VecDeque<(usize, usize)>> }
pub fn init_per_core_arenas(heap: HeapInfo) -> Result<(), &'static str>;
pub fn drain_remote_frees(cpu_id: u32);                   // chiamata su confini alloc/free
pub fn allocate_large_block(size: usize) -> Option<*mut u8>;
pub fn free_large_block(ptr: *mut u8, size: usize);
```
`GlobalAlloc` impl che instrada small→arena locale, large→pool globale, free→arena
proprietaria o remote-free queue.

**Touch points.** `memory/heap.rs` (sostituisci `ALLOCATOR`), `memory/mod.rs`
(re-export), `cpu/mod.rs` (slot arena in `PerCpu` o array parallelo),
`boot/phases/mem.rs` (chiama `init_per_core_arenas` dopo `init_heap`), `sync/mod.rs`
(eventuale helper remote-free).

**Test/accettazione.** Fast path small senza contesa lock (BSP mono). Free cross-core:
alloc su core 0, free su core 1, conferma drenaggio + riuso. OOM su un'arena → fallback
globale. Marker boot `heap arenas ok cores=N threshold=Y`. Latenze alloc/free non
spikano sotto submission di job multi-core.

**Rischi.** talc single-arena → serve impl slab/magazine nuova o arena-manager attorno
a talc. Imbalance arene (core 0 OOM mentre core 1 idle) — vedi §10 cross-step con Step 5
(GUI pinnata = pressione alloc asimmetrica). Free cross-core latenza/backlog. Fallback
large-block come collo a 10+ core.

---

## 7. Step 2 — Message bus inter-core (+ wake cross-core + tabella vettori IPI)

**Obiettivo.** Generalizzare il pool di job puri in un bus **richiesta/risposta async**
cross-core. Trasporta payload heap + waker di reply, non solo `fn(&[u8])->u64`. Include
la primitiva di wake cross-core (§3) e la **tabella di allocazione vettori IPI**.

**Stato attuale.**
- `smp::pool` (`pool.rs`): 64 slot, state machine CAS `EMPTY→QUEUED→RUNNING→DONE`,
  `QUEUE: IrqMutex<VecDeque<usize>>`, `JobFn` puro. Niente heap, niente waker.
- IPI wake: `send_ipi_all_but_self(VEC_WAKE=0x40)` via LAPIC ICR (`lapic.rs:77-85`);
  handler `VEC_WAKE` fa `eoi()` (`idt.rs:90-94`).
- Waker reference esistente: `wasm/exec_queue.rs` (`ExecFuture` con
  `Mutex<Option<Waker>>`).

**Target.**
- **Inbox per-core:** `static PER_CORE_INBOX: [PerCoreInbox; MAX_CPUS]`, ognuna
  `IrqMutex<VecDeque<Box<InboxMsg>>>`.
- `InboxMsg { payload: Box<[u8]>, reply_waker: Option<Waker> }` — payload opaco heap;
  il waker abilita l'await della reply sul core mittente.
- `core_send(target, msg)` → enqueue su `target` + `send_ipi(target, VEC_INBOX)`.
- Handler `VEC_INBOX` setta `INBOX_PENDING[target]`. Il loop del core (executor o GUI)
  drena `core_recv(cpu_id)`, esegue il payload (interpretato dall'owner), chiama
  `reply_waker.wake()` se presente → §3 sveglia il mittente.
- **Wake cross-core (§3):** `WAKE_PENDING` per-core + `__pender` mirato. Costruito QUI.
- **Tabella vettori IPI (gap §10):** budget unico e fisso, niente collisioni:
  `0x20` timer, `0x21` kbd, `0x22` mouse, `0x40` `VEC_WAKE`, `0x41` `VEC_INBOX`,
  `0x42` `VEC_TLB_SHOOTDOWN` (Step 3), `0x43` `VEC_RESET` (Step 6). Documentata in
  `idt.rs`.

**Nuove API.**
```
pub struct InboxMsg { payload: Box<[u8]>, reply_waker: Option<Waker> }
pub fn core_send(target: u32, msg: Box<InboxMsg>) -> Result<(), Box<InboxMsg>>;
pub fn core_recv(cpu_id: u32) -> Option<Box<InboxMsg>>;
pub const VEC_INBOX: u8 = 0x41;
extern "x86-interrupt" fn inbox_handler(_: InterruptStackFrame);
// wake cross-core (§3):
static WAKE_PENDING: [AtomicBool; MAX_CPUS];
pub fn wake_core(owner: u32);                 // set flag + IPI mirato se owner != self
```

**Touch points.** `smp/inbox.rs` (nuovo, sibling di `pool.rs` — non rifattorizzare il
pool), `idt.rs` (handler + tabella vettori), `apic/lapic.rs` (`send_ipi` mirato),
`cpu/ap.rs` + `executor/mod.rs` (drain inbox nel loop), `executor/mod.rs` (`__pender`
+ `WAKE_PENDING` per-core).

**Test/accettazione.** `core_send` prima che l'AP sia pronto → `Ok`; `core_recv` →
`None` poi `Some`. BSP→AP: AP modifica contatore via payload, chiama `reply_waker.wake`,
BSP fa await e riprende (prova end-to-end del wake cross-core). `inbox_handler` scatta
una volta per send. Stress: 64 enqueued, 65° `Err`, drena 32, retry ok. `core_send`
< 2 µs.

**Rischi.** Dip. da Step 1 per il payload (fallback `Box` globale → dip. **soft**, ma
il free cross-core del `Box` sul ricevente è dip. **hard** di correttezza, §10).
Waker lifetime/Send-Sync (§3, da verificare). Ordering publish/consume (invariante 9):
Release sull'enqueue, IPI come fence, Acquire sul recv. Vettore `0x41` da confermare
libero.

---

## 8. Step 3 — Executor per-core (+ timer AP + TLB shootdown)

**Obiettivo.** Ogni core esegue il proprio `RawExecutor` (run-queue + Delay list +
idle/hlt). Gli AP entrano in `run_core()` invece del solo `ap_worker_loop`. **Dip. HARD
da Step 1 e 2.** Include due gap di correttezza: **timer LAPIC per-AP** e **TLB
shootdown cross-core**.

**Stato attuale.**
- `EXECUTOR` singleton (`executor/mod.rs:45`, `ExecCell` con `unsafe impl Sync` gated su
  "solo il BSP chiama `run()`", `:31-43`). 10+ task nella sola coda BSP.
- `WAKE_PENDING` globale (`:24`); `__pender` globale (`:437`).
- `Delay`: `SLOTS_LIST` 64 slot `spin::Mutex`, drenata da `delay::timer_tick(now)` ad
  ogni IRQ timer; `GEN_COUNTER` guardia ABA (`delay.rs:36,47,151`). **Lato ISR esiste**
  (il timer ISR scansiona con `try_lock`-defer), lato task `without_interrupts`.
- Timer: maschera LVT su AP in `init_ap` (`lapic.rs:64-72`); solo il BSP riceve il
  tick → `timer_handler` → `delay::timer_tick` (`timer.rs:11-16`). Gli AP ricevono solo
  IPI di wake.

**Target.**
- `PER_CORE_EXECUTOR: [PerCoreExecutor; MAX_CPUS]` (`{ exec: RawExecutor, wake_pending:
  AtomicBool }`); `executor::run_core(cpu_id) -> !` rimpiazza `run()`.
- **Delay list per-core:** `PER_CORE_DELAYS[[Option<DelaySlot>; 64]; MAX_CPUS]`;
  `delay::timer_tick_core(now, cpu_id)`; `Delay::ticks` lega al `cpu_id` corrente.
- **Timer LAPIC per-AP (gap §10):** `init_ap` smaschera il timer LVT; gli AP ottengono
  un tick. **Calibrazione:** il BSP calibra contro ACPI PM/TSC e **pubblica le costanti
  condivise** (init-once, read-only); ogni AP programma il proprio LVT con quelle.
  `TICKS` resta **un solo globale** (invariante 8); l'ISR per-core walka solo la **sua**
  Delay list.
- **Cross-core spawn:** `PER_CORE_SPAWN_QUEUE[cpu_id]` (`IrqMutex`); `spawn_on(cpu_id,
  task)` enqueue + `wake_core(cpu_id)` (§3). Spawn locale = fast path via
  `executor[cpu_id].spawner()`.
- **TLB shootdown cross-core (gap §10 — correttezza):** quando il `MAPPER` condiviso
  (un PML4) flippa W^X o unmappa (teardown modulo WASM, DMA), un core che ha cachato un
  PTE stale userebbe traduzioni vecchie = **buco memory-safety silenzioso**. Serve un
  **protocollo IPI di shootdown**: il core che muta una mapping potenzialmente vista da
  altri invia `VEC_TLB_SHOOTDOWN` ai core interessati; l'handler fa `invlpg`/flush e
  conferma; il mittente attende le conferme prima di procedere. Tenere il `MAPPER` lock
  durante la sequenza ma **non** attraverso await (invariante 3).
- **`__pender` per-core (§3):** setta `WAKE_PENDING[owner]`, IPI mirato se cross-core.
- **Compatibilità pool:** gli AP ora sono executor, non worker puri. `smp::pool` resta
  come fallback per job puri (compositing bande), ma non è più l'unico ruolo AP.

**Nuove API.** `PerCoreExecutor`, `PER_CORE_EXECUTOR`, `run_core(cpu_id)`,
`timer_tick_core(now, cpu_id)`, `Delay::ticks` core-aware, `spawn_on(cpu_id, task)`,
`set_timer_periodic_ap(vector, count)`, `ap_entry_phase2(info) -> !` (entra in
`run_core`), `VEC_TLB_SHOOTDOWN=0x42` + `tlb_shootdown(range, cores)`.

**Touch points.** `executor/mod.rs` (per-core arrays, `run_core`, `__pender`),
`executor/delay.rs` (per-core lists, `timer_tick_core`), `cpu/ap.rs`
(`ap_entry_phase2`→`run_core`), `apic/lapic.rs` (smaschera timer AP + calibrazione
condivisa + `send_ipi` mirato), `timer.rs` (`timer_handler` per-core →
`timer_tick_core(now, cpu_id())`), `boot/phases/userland.rs` (`run()`→`run_core(0)`),
`memory/mapper.rs` (hook shootdown sulle mutazioni), `idt.rs` (`VEC_TLB_SHOOTDOWN`).

**Test/accettazione.** Marker `executor core N running` per core. Wake per-core:
task su core 0 in `Delay`, task su core 1 lo sveglia via `__pender` → core 0 esce da
hlt, nessun crosstalk. Timer per-core: ISR su tutti i core a 100 Hz, ogni Delay list
drena la propria; task Delay su più core dormono/svegliano indipendenti. Cross-core
spawn: `spawn_on(1, …)` da core 0 → marker `spawned on core=1`. TLB shootdown: mappa/
unmappa una pagina vista da 2 core, verifica che il core remoto NON usi la traduzione
stale (test mirato con pagina trap). Regressione: i 10 task BSP girano; cpustat
busy/idle indipendente per core.

**Rischi.** GDT/TSS/IDT per-core DEVONO essere installati in `ap_entry` prima di
`run_core` (lo sono, `cpu/ap.rs:20-21`). `__pender` dispatch errato (setta sempre core
0) → AP mai svegliati: verificare con test. `GEN_COUNTER` globale ora scritto da 16 ISR
timer concorrenti su 16 liste separate (§10 cross-step): valutare per-core. Waker embassy
legato a un executor (§3). TLB shootdown è la classe di bug più subdola — sintomo
lontano dalla causa.

---

## 9. Step 4 — Ownership dei globali RW-hot (in parallelo a Step 5)

**Obiettivo.** Dare un owner a ogni globale RW-hot; accesso cross-core via bus. **Dip.
da Step 2+3.** Decisioni utente baked-in (vedi §12).

**Stato attuale.** `NET: Mutex<Option<NetState>>` (`net/mod.rs:36`); `CONSOLE`
(`console/mod.rs:67`), `LOG` (`klog.rs:56`); `REGISTRY: Mutex<BTreeMap>` + `NEXT_PID:
AtomicU32` (`proc.rs:25-26`); `PAIRS[4]: [Mutex<PtyPair>; 4]` (`pty/mod.rs:12-17`);
`RNG: Mutex<Option<ChaCha20Rng>>` (`rng.rs:8`).

**Target (per sotto-sistema — decisioni §12).**
- **NET → owner = BSP.** `net_poll_task` resta sul BSP; gli altri core postano
  `SocketOp{op, fd, args}` all'inbox del BSP via `core_send`, await reply. Elimina N
  holder concorrenti di `NET.lock()`.
- **CONSOLE/LOG → log-core async.** `kprintln!` posta `Write{bytes}` all'inbox del
  log-core; il log-core batcha e prende `CONSOLE.lock()` una volta per batch.
  **Fallback sincrono obbligatorio** per il panic handler e l'early boot (prima che
  l'executor/log-core esistano): in quei contesti `kprintln!` usa `try_lock` + serial
  diretto. Doppio path esplicito.
- **PROC → registry per-core.** Ogni executor traccia i PID che possiede (`[Option<
  ProcInfo>; N]` locale). `ps`/`kill` cross-core = query via bus all'owner-core del PID.
  `NEXT_PID` resta `AtomicU32` globale (unicità semplice, contesa trascurabile — un
  increment per exec); range per-core SOLO se il profiling mostrasse contesa (improbabile).
- **RNG → replica per-core.** `RNG[MAX_CPUS]` ChaCha20; seed da entropy bank riempito
  una volta dal BSP (RDRAND). Ogni core tocca solo il suo slot → lock-free cross-core.
- **PTY → owner-per-coppia.** Coppia `i` → pty-core assegnato (round-robin a bind-time
  per evitare imbalance). keyboard ISR / sessioni SSH su altri core postano
  `PtyMasterInput{idx, byte}` all'owner. Atomici `CLAIMED/SHUTDOWN/LAST_ACTIVITY`
  restano cross-core (cheap, Relaxed).

**Nuove API.** `enum Message { SocketOp, Write, PtyMasterInput, RngRequest, ProcQuery,
… }`; `async fn send_message(core_id, msg) -> Reply`; service loop per owner
(`net_service_loop`, `log_service_loop`, `pty_service_loop`); `rng_fill(buf)` legge
`RNG[cpu_id()]`.

**Touch points.** `executor/mod.rs` (service loop per owner), `net/mod.rs`
(`NET.lock()`→`send_message(BSP, SocketOp)`; `net_poll_task` resta), `console/mod.rs` +
`klog.rs` (route a log-core + fallback sync), `proc.rs` (registry per-core + query),
`pty/mod.rs` (route master-input all'owner), `rng.rs` (per-core), `wasm/host/proc.rs`
(socket ops async via `send_message`), `kprint.rs` (async + fallback ISR/boot).

**Test/accettazione.** RNG per-core: output differisce per core, nessuna call
cross-core. PTY ownership: coppia 0→core owner, sessioni SSH su 2 core, master-input
instrada all'owner, niente deadlock su `set_foreground` cross-core. Boot 4 core: 2 shell
socket su 2 core, traffico loopback, latenza round-trip `send_message` misurata.
`kprintln!` da 4 core: ordine log coerente per owner singolo. **Accettazione:** `ps`,
`kill`, SSH, `dmesg` passano con SMP; nessun hang su timeout messaggi.

**Rischi.** Dip. hard da Step 2+3. `kprintln!` async che cede rompe i chiamanti che lo
assumono sincrono (panic/boot) → il **doppio path** è obbligatorio, non opzionale.
Lock-ordering nei message handler (se il net-handler prende `REGISTRY`...): audita
l'ordine. Imbalance assegnazione PTY→core: round-robin a bind-time. Latenza messaggi se
un core è occupato e non polla l'inbox: l'executor DEVE svegliarsi su messaggio (§3).
Registry per-core complica `ps`/kill globali (round-trip).

---

## 10. Step 5 — Pin workloads (GUI su core dedicato) + CORE_ROLES SSOT

**Obiettivo.** La vittoria visibile: compositor fluido su un core dedicato MENTRE
SSH/net/USB restano vivi. **Dip. da Step 2+3, indipendente da Step 4.** Possiede la
**tabella `CORE_ROLES` come autorità unica** (riconcilia i tassonomie ruoli di Step
4/5/6).

**Stato attuale.**
- `exec_worker_task` chiama `run_compositor_gate` **inline** (`executor/mod.rs:133`) →
  `Compositor::run() -> !` (`wm.rs:1096`) busy-spinna (`:1210`) e blocca l'executor.
- `fold_mouse` doppio-pompa `usb::poll()` (`gfx/mod.rs:235`) perché la GUI sync affama
  `usb_poll_task`.
- `dispatch_bands`/`composite_band_job` (`wm.rs:169-197`) già pool-parallelo,
  `&'static` via `BAND_ARENA`.
- `bringup` (`smp/mod.rs:8-69`) assegna `cpu_id` densi, nessuna tabella ruoli.

**Target.**
- **`CORE_ROLES: [AtomicU8; MAX_CPUS]`** (autorità unica), riempita a `bringup` dopo
  l'assegnazione `cpu_id`. `enum CoreRole { BspIo, GuiCompositor, LogOwner, PtyOwner,
  ComputeApp }`. `ap_entry` fa match su `core_role(cpu_id)` e dirama
  (`ap_worker_loop`/`run_core`/`gui_worker_loop`/service loop). Step 4 e 6 **leggono**
  questa tabella, non ne definiscono altre.
- **Offload compositor:** `exec_worker_task` ramo `compositor.cwasm`, se
  `cpus_online() >= 2`: leak `&[u8]` `'static`, pubblica nella mailbox del GUI-core
  (`CompositorMailbox { ptr, len, ready }`), IPI il GUI-core, **ritorna** (executor
  libero). Altrimenti (1 core): `run_compositor_gate` inline (oggi).
- **GUI-core entry:** `gui_worker_loop()` spinna su `mailbox.ready` (Acquire), carica
  ptr/len, azzera ready, chiama `run_compositor_gate`. Il `-> !` va bene (possiede il
  core). La mailbox è un canale Step-2 degenerato — **riusa la primitiva**, non un
  secondo canale ad-hoc.
- **USB owner dedup:** il BSP possiede `usb_poll_task` (ora non più affamato) →
  **rimuovi `usb::poll()` da `fold_mouse`** (`gfx/mod.rs:235`). La GUI consuma solo le
  code mouse/gfx (già `IrqMutex`, cross-core safe).
- **Degrado 2 core:** niente AP spare → le bande girano inline sul GUI-core (già il
  fallback documentato in `wm.rs`). **1 core:** gate inline sul BSP.

**Nuove API.** `enum CoreRole`, `CORE_ROLES`, `set_core_role`/`core_role`,
`CompositorMailbox` + `COMPOSITOR_MAILBOX`, `gui_worker_loop() -> !`,
`send_compositor_to_core(cwasm) -> bool`.

**Touch points.** `smp/mod.rs` (`bringup` assegna ruoli + mailbox + `send_compositor_
to_core`), `cpu/ap.rs` (`ap_entry` match ruolo + `gui_worker_loop`), `executor/mod.rs`
(`exec_worker_task:132-134` → `send_compositor_to_core`, ritorna se offload riuscito),
`gfx/mod.rs:235` (rimuovi `usb::poll()`), `cpu/mod.rs` (`CORE_ROLES` + accessor).
`wm.rs` invariato (dispatch_bands + run_compositor_gate non cambiano).

**Test/accettazione.** Marker boot tabella `CORE_ROLES`. Bande ancora su compute core
(o GUI-core se niente spare) via `COMPOSITE_CORE_MASK`. **Compositor fluido**: frame
time stabile mentre SSH risponde subito (`usb_poll_task` non più affamato). Mouse/
tastiera real-time in GUI, SSH non freeza. Fallback 2 core: bande inline, niente
deadlock. Fallback 1 core: boota (lento).

**Rischi.** Il busy-spin del compositor (~1 ms/frame) tiene il GUI-core occupato — ok,
è dedicato. Cache coherency mailbox: atomici + IPI come fence (invariante 9), SeqCst su
ptr/len/ready, `ready` settata DOPO ptr/len. Assegnazione ruolo statica a bringup
(no NUMA-aware) — accettabile per Step 5; Step 6 può aggiungere bilanciamento.
`BAND_ARENA` `&'static` quando il GUI-core fa sia compositor sia bande inline (§10
cross-step): auditare lifetime/aliasing nell'hand-off.

---

## 11. Step 6 — Supervisore + panic per-core

**Obiettivo.** Solidità senza preemption: confinamento (un core ostile freeza solo sé)
+ supervisione (heartbeat per-core + restart). **6-detect anticipabile; 6-recover dip.
da Step 3+4.**

**Stato attuale.** `cpustat CORE[16]` (`cpustat.rs:14-26`) — solo TSC, nessun
heartbeat. Panic handler **globale** (`main.rs:111-169`): un panic = macchina giù.
Watchdog solo PTY (`executor/mod.rs:395-419`). Fuel WASM (`wasm/fiber.rs:25-181`),
nessun deadline sui job kernel.

**Target.**
- **Heartbeat per-core:** `CORES[cpu_id].heartbeat: AtomicU64` bumpato nel main loop di
  ogni core (executor poll / AP pool drain). Solo atomici, nessun lock nuovo.
- **Supervisor task** (su un core diverso da quello osservato): legge gli heartbeat;
  se un core è muto > T (es. 5 s = 500 tick) → logga + (se AP) setta
  `restart_requested` / killa l'istanza WASM colpevole; se BSP muto → reboot (non può
  uccidere sé).
- **Panic per-core:** `#[panic_handler]` resta unico (Rust ne ammette uno), ma fa
  dispatch su `cpu_id()`: BSP → comportamento attuale (halt/reboot); AP → logga su
  serial, azzera il proprio heartbeat (il supervisor vede il muto), `cli;hlt` locale →
  un panic AP non porta giù la macchina.
- **Deadline job kernel:** opzionale timeout in `pool::submit`; il fuel WASM già uccide
  le app che non cedono.

**Dipendenza chiave (cross-step §10):** il supervisor è un task async; **non può girare
se l'executor che lo ospita è bloccato.** Finché il compositor blocca il BSP (pre-Step
5), il supervisor è affamato. Quindi 6 è significativo **dopo** Step 5 (compositor su
core suo) — arco di ordinamento hard.

**Nuove API.** `sched/core.rs` (nuovo): `CoreState { heartbeat, restart_requested,
panic_count }`, `CORES[MAX_CPUS]`, `heartbeat_tick(cpu_id)`, `mark_restart_requested`/
`is_restart_requested`; `supervisor_task()`; `per_core_panic_recovery(cpu_id)`;
`SUPERVISOR_TIMEOUT_TICKS=500`.

**Touch points.** `sched/mod.rs` (+`mod core`), `sched/core.rs` (nuovo),
`executor/mod.rs` (heartbeat dopo poll + spawn supervisor + `supervisor_task`),
`cpu/ap.rs` (heartbeat dopo job + check `is_restart_requested` prima di hlt),
`main.rs` (panic handler → `per_core_panic_recovery(cpu_id())`).

**Test/accettazione.** Heartbeat incrementa per core. Halt artificiale di un AP →
supervisor logga `core N mute for Xs` entro ~6 s, **niente reboot macchina**. Panic su
AP → recovery locale, BSP+altri continuano. Panic su BSP → reboot. Compositor attivo →
supervisor sveglia comunque ogni 1 s (DOPO Step 5).

**Rischi.** Starvation supervisor se l'host executor è bloccato (→ ordina dopo Step 5).
Uccidere istanze su AP muto richiede il **registry per-core** (Step 4): 6-detect logga,
6-recover uccide. Panic dentro il panic handler (`cpu_id()` deve essere infallibile —
lo è, atomic load). Falsi positivi se il kernel si ferma > 5 s intenzionalmente (non
deve).

---

## 12. Conflitti risolti (decisioni utente — baked-in)

| Conflitto | Decisione | Note |
|---|---|---|
| **Ruoli core** | **Ownership distribuita** (net/log/pty owner separati) | con NET pinnato sul BSP (vedi sotto); degrada a BSP-unico sotto 4 core |
| **CONSOLE/LOG** | **log-core async** | `kprintln!` posta messaggi; **fallback sync obbligatorio** per panic/early-boot (doppio path) |
| **PROC REGISTRY** | **registry per-core** | ogni core traccia i suoi PID; `ps`/kill cross-core via bus |
| **NET** | **BSP possiede NET** | net-owner = BSP (affinity con usb/ssh); altri core via `SocketOp` sul bus |
| **RNG** | per-core ChaCha replica | adottato d'ufficio (nessun trade-off reale) |

**Riconciliazione conflitto-1 × conflitto-4:** "ownership distribuita" in generale, ma
**NET specificamente pinnato sul BSP**. log-core e pty-core sono owner separati quando
≥4 core; sotto, collassano sul BSP. `CORE_ROLES` (Step 5) è l'autorità unica che
implementa questa assegnazione dinamica.

---

## 13. Gap di correttezza trasversali (promossi a prima classe)

Il critico ha trovato sei elementi che il north-star §4 non possedeva. Riepilogo con
l'owner di step (dettaglio nei rispettivi step):

1. **TLB shootdown cross-core** (Step 3) — MAPPER condiviso flippa W^X/unmappa → TLB
   stale = buco memory-safety silenzioso. Protocollo IPI `VEC_TLB_SHOOTDOWN`.
2. **Wake/`__pender` cross-core** (Foundation §3, costruito in Step 2/3) — primitiva di
   wake unica, oggi globale. È IL vero gap foundational.
3. **Tabella vettori IPI** (Step 2) — budget unico `0x20/21/22/40/41/42/43`, niente
   collisioni.
4. **Calibrazione timer LAPIC su AP** (Step 3) — senza tick AP, le Delay list per-core
   non scattano. Costanti condivise dal BSP.
5. **Waker Send/Sync cross-core** (Foundation §3, verifica in Step 2/3) — `.wake()` da
   un altro core safe by construction (solo `__pender`, mai mutazione dell'executor
   remoto); da confermare con test.
6. **`CORE_ROLES` single source of truth** (Step 5) — Step 4/5/6 leggono una sola
   tabella, non tre tassonomie.

**Rischi cross-step (interazioni da tenere d'occhio):**
- **Free cross-core (Step 1) × InboxMsg Box (Step 2):** il `Box<[u8]>` inviato A→B e
  droppato sul ricevente è esattamente il caso free-cross-core → Step 1 (remote-free
  queue) è dip. di **correttezza** di Step 2, non ottimizzazione.
- **TLB coherence (gap 1) × MAPPER condiviso (Step 7) × WASM per-core (Step 3/4):**
  spazia su 3 step + il gap; va trattato come unità.
- **Supervisor starvation (Step 6) × compositor bloccante (Step 5):** 6 significativo
  solo dopo 5.
- **Heap imbalance (Step 1) × GUI pinnata (Step 5):** pressione alloc asimmetrica rende
  l'imbalance arene probabile, non teorico → considera rebalancing/remote-alloc.
- **GEN_COUNTER globale (delay) × timer AP (Step 3):** 16 ISR timer concorrenti su 16
  liste → profilo da single-writer a multi-writer; valuta per-core.

---

## 14. Trade-off / limiti onesti

- **Confinamento, non immunità.** Un task che busy-loopa freeza il suo core (gli altri
  vivono). La preemption darebbe immunità; il prezzo (ring 3, context-switch) è fuori
  dal pivot.
- **Latenza messaggi.** Ciò che era una call locale (es. `read` VFS, op socket) diventa
  un round-trip inter-core quando l'owner è altrove. Mitigazione: affinity + batch.
- **Disciplina.** Shared-nothing è **per disciplina**, non enforced (niente MMU tra
  core). Un bug su core A può corrompere lo stato "posseduto" da B. È il rischio
  strutturale del §8 del north-star doc; il completamento elegante (WASM come
  isolamento universale, §9 del north-star) è **fuori scope** di questa spec ma resta la
  direzione.
- **Costo `cpu_id()` sul path caldo** (Step 1) — **MISURATO e RISOLTO**: ~200 ns
  (LAPIC MMIO) confermati dallo spike; soluzione **Step 1a RDTSCP+TSC_AUX** (verificata
  portabile incl. VirtualBox; gs-base scartato). Non più un trade-off aperto.
- **Coerenza memoria arene** — free cross-core richiede cura (remote-free queue).

## 15. Cosa NON fare (anti-pattern)

- **NO scheduler preemptive + ring 3 + page-table per-processo.** Tradisce il pivot.
- **NO `IrqMutex` su ogni globale "per SMP-safety".** I lock sono già SMP-safe; il
  problema è la **contesa**, non la safety. Aggiungere lock sposta le race a runtime,
  l'ownership+messaggi le elimina by construction.
- **NO secondo canale di wake ad-hoc** per step. Una sola primitiva (§3).
- **NO tre tassonomie di ruolo core.** Una sola `CORE_ROLES` (Step 5).

## 16. Strategia di test complessiva

- **Per-step:** marker boot greppabili (come `composite cores={N}` esistente) +
  `boot-checks` self-test in-boot.
- **Multi-core reale:** `make run-test` con `QEMU -smp 4` (oggi il default potrebbe
  essere 1 — bumpare per i test SMP). VirtualBox + HW reale per i quirk (gs-base,
  cpu_id LAPIC-based).
- **Suite esistenti** (run-test, run-ssh-test, run-pipe-test, run-fuel-test) verdi a
  ogni step (no regressione single-core).
- **Test di correttezza dedicati** per i gap: wake cross-core end-to-end, TLB shootdown
  con pagina trap, free cross-core con riuso, ordering mailbox/inbox.

## 17. Riferimenti codice (ancore)

- Lock/sync: `sync/mod.rs:14-72`; audit baseline `CHANGELOG/186`.
- Executor singleton + `__pender` + WAKE: `executor/mod.rs:24,31-43,45,51,113,133,437`.
- Delay: `executor/delay.rs:36,47,151`.
- SMP bring-up + AP loop: `smp/mod.rs:8,50`, `cpu/ap.rs:17,35-58`.
- Compute pool: `smp/pool.rs:50,54,79`.
- IPI: `apic/lapic.rs:77-85`; handler `idt.rs:90-94`.
- Per-CPU: `cpu/mod.rs:27-34,50,127-136`; GDT/TSS per-core `gdt.rs:21-31`.
- Allocatore: `memory/heap.rs:19`; frame `memory/frames.rs:144`; mapper
  `memory/mapper.rs:13-14`; exec W^X `memory/exec.rs`.
- Compositor: `wasm/wt/wm.rs:1096,1169-197,1210,1217`; gfx `gfx/mod.rs:14-24,191,235`.
- Globali subsystem (Step 4): `net/mod.rs:36`, `console/mod.rs:67`, `klog.rs:56`,
  `proc.rs:25-26`, `pty/mod.rs:12-17`, `rng.rs:8`.
- Supervisor base: `cpustat.rs:14-27`; panic `main.rs:111-169`; fuel
  `wasm/fiber.rs:25-181`.
- North-star doc padre: `docs/superpowers/specs/2026-06-05-smp-shared-nothing-architecture-design.md`.
