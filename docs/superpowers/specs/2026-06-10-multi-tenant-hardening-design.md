# Multi-tenant hardening — design

**Data:** 2026-06-10
**Stato:** PROPOSTA (nessun codice ancora)
**Branch previsto:** dedicato (non `feat/usb-wifi-rtl8188eu`)

## Contesto e threat model

Aggiornamento di rotta rispetto al pivot 2026-05-28: in un futuro prossimo ruos
dovrà eseguire **codice WASM di terzi non fidato** ed essere **multi-utente**
(SSH). Oggi l'isolamento è interamente software (runtime WASM in ring 0,
single address space): un bug di memory-safety in wasmtime/wasmi o in un host
fn = compromissione totale, e un guest ostile può monopolizzare il BSP
(freeze GUI/rete/USB) o tentare letture speculative (Spectre v1) della
memoria kernel.

**Vincoli architetturali confermati dall'utente (2026-06-10):**

- **NO ring 3** (resta il pivot).
- **NO page table per-istanza WASM** (resta il drop "no per-process page
  tables" — nemmeno nella variante ring-0-only).
- **NO PKS** come requisito (taglierebbe fuori hardware pre-Alder-Lake).
- PKU ammesso **solo feature-detect**: attivo dove il silicio lo supporta,
  tier software-only altrove.

Conseguenza accettata e documentata: su hardware senza PKU l'isolamento del
codice di terzi si regge interamente su correttezza del runtime + mitigazioni
Spectre. Decisione consapevole, non difetto nascosto.

## Stato di fatto verificato nel codice (2026-06-10)

| Fatto | Dove |
|---|---|
| Bounds check **inline** nel codice AOT (niente guard page) | `signals_based_traps(false)` + `memory_guard_size(0)` in `kernel/src/wasm/wt/mod.rs:285,294` e `tools/wt-precompile/src/main.rs:39,42` (config hashata nel `.cwasm`) |
| Demand paging linear memory **già esiste** | `kernel/src/wasm/wt/demand.rs` (frame al touch, 256 MiB VA riservata) |
| Fuel wasmi = **watchdog kill**, non quantum | `kernel/src/wasm/fiber.rs:91-93` — refuel a ogni host call; solo tight loop pure-compute brucia 2G e viene ucciso |
| Nessuna epoch interruption | zero hit `epoch_interruption` nel repo |
| `frame()` compositor **senza deadline** | `wm.rs:1518-1522` (`frame.call`); un `frame()` runaway blocca il BSP per sempre. L'`Err` path esiste già: trap → `close_requested` → reap |
| Tool `.cwasm` da shell senza deadline | `wt/mod.rs:346` (`run.call`) |
| Componenti TUI senza deadline né kill (agg. 2026-06-11) | `wt/component.rs::run_tui_component` (`run.call` dell'app + shim canvas → provider): nessun fuel, nessun check `is_kill_pending` → `kill <pid>` ignorato; runaway tiene il core AP per sempre. In più `poll_key` spin-wait al 100% sull'AP per design |
| wt-precompile non tocca i flag Spectre di Cranelift | nessun hit `spectre` in `tools/wt-precompile` → default Cranelift (mitigazione heap-access ON di default — da confermare in SP2) |
| Timer 100 Hz BSP+AP | `kernel/src/timer.rs:27` `timer_handler`, vettore `idt.rs:10` |

## Sottoprogetti

Ordine = priorità. SP1 e SP2 servono **anche oggi** (freeze GUI, robustezza);
SP3-SP5 diventano necessari all'arrivo del multi-tenant.

---

### SP1 — Epoch watchdog Wasmtime (anti-monopolio BSP)

**Obiettivo:** nessun guest Wasmtime può tenere il BSP oltre un budget di
tempo. Trap → l'infrastruttura di errore esistente lo gestisce (finestra
reaped / tool terminato). Niente yield cooperativo: il compositor è reactor
senza fiber per scelta (contract doc 2026-06-05) — qui si fa solo **kill**,
non sospensione.

**Design:**

1. **Config (hashata!):** `config.epoch_interruption(true)` in *entrambi*
   `tools/wt-precompile/src/main.rs` e `engine_config()` in
   `kernel/src/wasm/wt/mod.rs`. L'epoch check è iniettato dal codegen ai
   function entry e ai loop backedge → cambia il settings-hash → **tutti i
   `.cwasm` vanno ricompilati** (gate: il mismatch hash dà errore esplicito
   al load, non corruzione silente).
2. **Tick:** `ENGINE.increment_epoch()` dal `timer_handler`
   (`kernel/src/timer.rs:27`), **solo BSP** (gli AP hanno lo stesso vettore:
   gate su core-id LAPIC, altrimenti il rate si moltiplica per N core).
   `increment_epoch` è un incremento atomico → IRQ-safe. Engine globale:
   serve un riferimento `'static` all'`Engine` (oggi creato per-store? da
   verificare in implementazione — se per-store, registrare gli engine vivi
   in una lista statica o passare a engine unico condiviso).
3. **Policy compositor (`frame_all`, `wm.rs:1509`):** prima di ogni
   `frame.call`: `store.set_epoch_deadline(FRAME_BUDGET_TICKS)` con
   `epoch_deadline_trap()` (default). Budget proposto: **5 tick = 50 ms**
   (3 frame compositor mancati, tollera un frame egui pesante ma uccide il
   loop infinito in <100 ms percepiti). Il trap rientra nel ramo `Err` già
   esistente → `close_requested = true` → reap. Stessa cosa per `init`
   (`wm.rs:938`) e per il gate (`mod.rs:256,346`).
4. **Policy tool `.cwasm` da shell (`run_cwasm`):** qui il guest può
   legittimamente calcolare a lungo. Usare `store.epoch_deadline_callback`:
   a ogni scadenza (es. ogni 10 tick = 100 ms) il callback controlla il flag
   **kill/Ctrl-C** della sessione (VINTR — il meccanismo foreground-kill di
   rtop esiste già lato PTY) e ritorna `UpdateDeadline::Continue(10)` oppure
   trap. Bonus immediato: **Ctrl-C uccide anche i `.cwasm` in tight loop**,
   cosa oggi impossibile.
5. **Policy componenti TUI (`run_tui_component`, `wt/component.rs` — agg.
   2026-06-11):** stesso `epoch_deadline_callback` dei tool (punto 4) sullo
   store dell'app: a ogni scadenza (~10 tick) controlla `is_kill_pending`
   del pid + VINTR e ritorna `Continue` o trap. Risolve il runaway loop che
   tiene il core AP per sempre. Il deadline copre anche le call nel provider
   via shim canvas (stesso store).
   **GIÀ FATTO in anticipo (CHANGELOG 429):** check `is_kill_pending` dentro
   il loop di `poll_key` + `set_foreground` sull'exec parallelo (`kill`/
   `pkill` funzionano, e2e-verificato via console sendkey) e attesa `sti;hlt`
   al posto dello spin (qemu ~5% CPU con rtop idle). Resta SOLO il caso
   runaway-senza-host-call = esattamente l'epoch-trap di questo punto.
6. **wasmi invariato:** fuel già fa da watchdog per i tool interpretati.

**Rischi/limiti:**

- Overhead epoch check: pochi % su loop stretti (un load+cmp+branch ben
  predetto per backedge). Misurare con `smptest`/app egui prima/dopo.
- Il deadline scatta solo a function entry/backedge: un host fn bloccante
  non è interrotto (ma gli host fn sono nostri, non del guest).
- `wasmtime = "=45.0.0"` runtime-only no_std: verificare che la feature
  epoch non richieda `std` (atomics puri — atteso OK, è il primo gate).

**Test:**

- Nuova app finestra `spin.wasm` (loop infinito in `frame()`): la finestra
  deve sparire entro ~100 ms e il desktop restare fluido.
- Tool `spin.cwasm` da shell: Ctrl-C lo termina; senza Ctrl-C continua.
- Componente TUI: rtop interattivo via SSH, `kill <pid>` da seconda sessione
  → rtop esce e ripristina il terminale; variante runaway (loop senza host
  call in `run`) → trap entro ~100 ms, AP libero.
- Regressione: `make run-test`, GUI demo, rtop (`make run-rtop-test`).

---

### SP2 — Verifica mitigazioni Spectre (solo audit + fix config, no feature)

**Obiettivo:** confermare che il codice AOT generato abbia il masking
Spectre sui bounds check e che il suo presupposto (pagina 0 non mappata)
valga in ruos.

**Checklist:**

1. Cranelift `enable_heap_access_spectre_mitigation` e
   `enable_table_access_spectre_mitigation`: default ON — confermare che
   wt-precompile non li spenga (oggi non li tocca) e **fissarli
   esplicitamente** a `true` nel sorgente con commento, così un upgrade di
   wasmtime non li cambia in silenzio.
2. Il masking Spectre fa cmov dell'indirizzo OOB a **0**: l'efficacia
   richiede che la **VA 0 non sia mappata** (né leggibile speculativamente).
   Verificare nel paging ruos che la pagina 0 sia non-presente; se l'identity
   map iniziale la copre, escluderla.
3. wasmi (interprete): bounds check nel loop dell'interprete; esposizione
   speculativa molto più bassa (niente gadget compilato controllabile) —
   solo nota, nessuna azione.
4. Audit host fn: con codice di terzi ogni `func_wrap` è confine di
   fiducia. L'accessor unico auditato (`host/mem.rs`) copre wasmi; estendere
   la stessa disciplina alle superfici Wasmtime `wm`/`sys`/`term`
   (`kernel/src/wasm/wt/*`): nessuna lettura guest fuori dall'accessor,
   nessun panic su input malformato (fuzz leggero con input limite).

**Limite dichiarato:** senza address space separati Spectre v1 non è
eliminabile al 100%; il masking chiude il gadget principale (bounds check
bypass). Residuo accettato per i vincoli di cui sopra.

---

### SP3 — Quote risorse per istanza Wasmtime

**Obiettivo:** un guest non esaurisce RAM/finestre.

- `ResourceLimiter` sugli store Wasmtime (oggi solo wasmi lo ha):
  cap `memory_growing` per finestra/tool (proposta: 64 MiB default,
  configurabile), cap tabelle/istanze.
- Cap numero finestre per sessione (launcher) e per utente (SSH).
- Trap/diniego pulito al superamento, non panic.

---

### SP4 — PKU feature-detect (porta d'acciaio dove il silicio c'è)

**Solo sketch — spec dedicata quando si parte.** Hardware: Intel client 11th
gen+ (~2020), AMD Zen 3+ (~2020), Xeon Skylake-SP+. Caveat Meltdown-PK su
Intel pre-Ice-Lake (bypass speculativo PKU): la barriera è piena solo su
silicio recente.

- Detect: CPUID.7.0:ECX.PKU[3] + abilitare `CR4.PKE`. Assenza → tier
  software (SP1-SP3), nessun costo.
- Le pagine della linear memory WASM (finestra WT + demand pages) marcate
  **U=1** con protection key K1; tutto il resto key 0.
- Entrata guest: `WRPKRU` nega accesso a key 0 → il guest (ring 0 ma che
  accede solo a pagine U=1/K1) non tocca la memoria kernel anche se il
  runtime ha un bug. Uscita guest / trap / IRQ: ripristino PKRU (entry IRQ
  comuni: salvare/ripristinare PKRU nello stack frame o forzarlo all'entry).
- Complicazioni note da affrontare nella spec dedicata: SMAP (accessi kernel
  a pagine U=1 richiedono STAC/CLAC o SMAP off), accesso host fn alla linear
  memory (host gira con key 0 piena → ok), demand paging #PF dentro il
  dominio guest (handler deve ripristinare PKRU), AP/SMP (PKRU è per-core).

---

### SP5 — Multi-user (fuori scope qui)

Ownership/permessi VFS per-utente, namespace FS per sessione SSH, quote
CPU/RAM per utente, separazione PTY. **Spec separata**; sensato solo dopo
SP1-SP3 (i permessi senza watchdog+quote sono aggirabili banalmente).

## Ordine e gating

```
SP1 (epoch watchdog)  ── gate: build no_std ok → test spin.wasm/spin.cwasm
SP2 (audit Spectre)   ── indipendente, può andare in parallelo a SP1
SP3 (quote)           ── dopo SP1 (condivide i punti di config store)
SP4 (PKU)             ── spec dedicata, quando il multi-tenant si avvicina
SP5 (multi-user VFS)  ── spec dedicata, dopo SP1-SP3
```

Kani sui path critici (`host/mem.rs`, `wt/demand.rs`) = workstream parallelo
opzionale, non bloccante.
