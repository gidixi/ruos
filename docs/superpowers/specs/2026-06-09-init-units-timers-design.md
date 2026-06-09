# Init system — units, timers, supervisione — design

**Data:** 2026-06-09
**Topic:** sistema di init nativo ruos (stile systemd+cron, non copia Linux):
unit con tipo/restart/dipendenze, timer (interval + calendario RTC), supervisione
daemon, config su file (YAML-subset + JSON), CLI `unitctl`.
**Repo:** ruos (kernel, padre).
**Progetto unico, multi-step** (una spec, un piano a fasi).

## Problema

Esiste già un **service manager** minimale (`kernel/src/service/mod.rs`): registry di
unit nominate (wasm/builtin), `service start/list/status`, un `service_dispatcher_task`
che esegue il fiber inline a completamento, stato Idle/Running/Exited/Failed, flag
`on_boot`, run counter. Manca: esecuzione **temporizzata** (cron), distinzione
**oneshot/daemon**, **restart** policy, **dipendenze/ordine**, **stop**, config su file.

Obiettivo: un init system completo che attivi unit a tempo (intervallo monotono +
calendario RTC) e a fasi (boot / post-boot / manuale), supervisioni i daemon
(restart con backoff, stop cooperativo), risolva dipendenze, con definizioni su file
persistenti + CLI runtime. Coerente col modello ruos: async cooperativo,
thread-per-core / **shared-nothing**, owner BSP + message-passing cross-core.

## Decisioni (da brainstorming)

- **Approccio A**: estendere il service manager in "unit manager" + 2 task async
  (supervisor, scheduler), riusando dispatcher/exec esistenti.
- Config **ibrida**: file `.unit` persistenti su disco + CLI runtime.
- Feature: **oneshot/daemon**, **restart** (no/on-failure/always), **deps**
  (after/requires), **timer** (interval + calendario semplificato).
- **Stop daemon cooperativo** (`proc::request_kill`, best-effort) + restart automatico.
- Calendario **semplificato** (hourly/daily/weekly/every/boot+).
- **Due formati** config: YAML-subset + JSON, parser hand-written (no crate no_std YAML
  affidabile per `x86_64-unknown-none`), modello interno comune `UnitDoc`.

## Architettura

```
boot (BSP)
  ├─ service::init()                 # builtin (ssh) seeded in codice
  ├─ service::load_from_disk()       # parse /mnt/etc/units/*.{yaml,json} → UNITS/TIMERS
  ├─ service::activate_target(Boot)  # topo-sort + start unit boot
  ├─ (spawn shell/desktop)
  └─ service::activate_target(PostBoot)

executor (BSP):
  service_dispatcher_task   # oneshot inline (esistente, esteso)
  supervisor_task           # daemon: runner leggero + restart policy
  scheduler_task            # polling 1s: timer due → start

exec routing (riusato):
  .cwasm → exec_cwasm_parallel → pick_compute_core() → ComputeApp core (AP)
  .wasm  → wasmi fiber → BSP

CLI unitctl (.wasm) ── host fn ruos: unit_* / timer_* ── UNIT_QUEUE (BSP) + registry
```

### 1. Modello Unit & Timer

In `service/mod.rs` (unit manager). Stringhe `String` (vengono da file).

```rust
pub enum UnitKind { Oneshot, Daemon }
pub enum RestartPolicy { No, OnFailure, Always }
pub enum ActivateTarget { Boot, PostBoot, Manual }

pub struct Unit {
    pub name: String,
    pub path: String,                 // "/mnt/bin/foo.wasm|.cwasm" | "<builtin>"
    pub kind: UnitKind,
    pub restart: RestartPolicy,
    pub after: Vec<String>,           // ordine
    pub requires: Vec<String>,        // pull-in a catena
    pub target: ActivateTarget,
    pub enabled: bool,
    pub status: UnitStatus,           // Idle/Running/Exited(c)/Failed(&str)/Restarting
    pub pid: Option<u32>,
    pub runs: u32,
    pub restarts: u32,                // per backoff
    pub stop_requested: bool,         // stop manuale → niente restart
}

pub enum Schedule {
    EveryTicks(u64),                  // intervallo monotono (OnUnitActiveSec)
    BootPlus(u64),                    // one-shot a boot+N tick
    Hourly { minute: u8 },
    Daily  { hour: u8, minute: u8 },
    Weekly { dow: u8, hour: u8, minute: u8 },  // dow: 0=Sun..6=Sat
}

pub struct Timer {
    pub name: String,
    pub unit: String,                 // unit attivata allo scatto
    pub schedule: Schedule,
    pub enabled: bool,
    pub next_fire: u64,               // epoch (calendario) o tick (monotono)
    pub last_fire: Option<u64>,
}
```

Due registry `UNITS`/`TIMERS` dietro `spin::Mutex<Vec<_>>` (come l'attuale `REGISTRY`),
sezioni critiche minime, mai `.await` sotto lock.

### 2. File config (YAML-subset + JSON)

Percorso: `/mnt/etc/units/*.{yaml,json}` (FAT32, unico FS persistente).

Service (YAML-subset):
```yaml
name: sshd
type: daemon            # oneshot | daemon
exec: /mnt/bin/sshd.wasm
restart: on-failure     # no | on-failure | always
target: boot            # boot | post-boot | manual
enabled: true
after: [net]
requires: [net]
```

Timer (`kind: timer`):
```yaml
name: backup
kind: timer
unit: backup-job
schedule: daily 03:00   # hourly :MM | daily HH:MM | weekly Mon HH:MM | every 300s | boot+10s
enabled: true
```

Stesse chiavi in JSON:
```json
{ "name":"sshd","type":"daemon","exec":"/mnt/bin/sshd.wasm","restart":"on-failure",
  "target":"boot","enabled":true,"after":["net"],"requires":["net"] }
```

**Parser** (hand-written, zero dep):
- `service/yaml.rs`: line-based — `key: value`, liste `[a, b]`, `#` commenti, salta vuote.
- `service/json.rs`: subset JSON — `{}`, `[]`, stringhe quotate, bool, numeri.
- Entrambi → **`UnitDoc`** (mappa key→`Val { Str|Bool|List(Vec<String>) }`).
- `service/unitfile.rs`: builder `UnitDoc → Unit|Timer` (estensione + `kind` discriminano),
  defaults (type=oneshot, restart=no, target=manual, enabled=false), validazione.

**`schedule_parse(&str) -> Schedule`**: parsing della stringa schedule.

**Robustezza**: chiave sconosciuta → warn; file malformato → unit `Failed(parse)` + log,
le altre proseguono (no boot-loop); dir assente → solo builtin.

### 3. Supervisor (lifecycle)

`supervisor_task` (BSP). NON esegue wasm inline — delega al routing exec esistente.

**Oneshot**: via il dispatcher esistente (`fb.run().await` → `mark_exited`). Restart
applicato solo se policy `on-failure`(code≠0)/`always` (ri-enqueue dopo backoff).

**Daemon**: ogni daemon = **runner leggero** (task BSP da pool `pool_size = MAX_DAEMONS`,
es. 8). Il runner:
```rust
loop {
    mark_running(name, pid);
    let code = match ext {
        ".cwasm" => exec_cwasm_parallel(path, argv, ...).await,  // → compute core (AP)
        ".wasm"  => run_wasmi_fiber(path, ...).await,            // → BSP
    };
    mark_exited_or_failed(name, code);
    if stop_requested { break; }
    match (policy, code) {
        (Always, _) | (OnFailure, c) if c != 0 => {
            backoff(restarts).await; restarts += 1; mark_restarting; continue;
        }
        _ => break,
    }
}
```
- **Backoff** esponenziale capato: 1s,2s,4s…max 30s (`Delay::ticks`). `restarts` reset se
  il daemon resta su > soglia (es. 60s) → recupera da crash transitori, evita crash-loop.
- **Stop manuale** (`unit stop`): `proc::request_kill(pid)` (cooperativo) + `stop_requested=true`
  → il runner non riavvia all'uscita. Best-effort (daemon in CPU-loop puro non si ferma —
  documentato; serve un check ai punti di yield delle host fn).
- Oltre `MAX_DAEMONS` → `start` fallisce con `NoSlot`.

**Placement esecuzione**: ereditato dal routing per-formato — `.cwasm` su compute core
(`pick_compute_core`, parallelo), `.wasm` su BSP (wasmi). Un daemon che vuole
parallelismo reale si shippa come `.cwasm`. Il runner BSP è leggero (await + policy).

### 4. Scheduler (timer)

`scheduler_task` (BSP), **polling 1s** (robusto a cambi RTC / timer runtime / drift):
```rust
loop {
    Delay::ticks(100).await;                 // ~1s
    let ticks = timer::ticks();
    let now = rtc::now(); let epoch = rtc::to_unix_epoch(&now);
    for t in TIMERS (enabled) {
        let due = match t.schedule {
            EveryTicks|BootPlus => ticks >= t.next_fire,
            _                   => epoch >= t.next_fire,   // calendario
        };
        if due {
            service::start(t.unit);          // attiva (qualunque target)
            t.last_fire = Some(/*now*/);
            t.next_fire = compute_next(t.schedule, now);   // prossima FUTURA
            if matches!(t.schedule, BootPlus(_)) { t.enabled = false; } // one-shot
        }
    }
}
```

**`compute_next(Schedule, now: RtcTime) -> u64`** (pura, `now` iniettato):
- `Hourly{m}` → prossima ora al minuto m. `Daily{h,m}` → oggi h:m se futuro, sennò domani.
- `Weekly{dow,h,m}` → prossimo giorno-settimana. `EveryTicks(n)` → ticks+n. `BootPlus` → armato una volta.
- Gestisce rollover ora/giorno/mese/anno (riusa la conversione Gregoriana di `rtc.rs`).

**Granularità 1s** (cron-like). **Catch-up**: "fire if due, recompute to FUTURE" → niente
doppio-scatto. **No backfill** di scatti persi a macchina spenta (YAGNI).

### 5. SMP / concorrenza (shared-nothing)

- **Owner = BSP**: registry + scheduler + supervisor + dispatcher sul BSP. Niente contesa
  cross-core sull'orchestrazione.
- **Richieste cross-core** (host fn CLI da un fiber app su un compute core): NON mutano i
  registry; **postano** in `UNIT_QUEUE` + svegliano il manager BSP (pattern `service::start`).
- **Letture** (`list`/`status`/`timers`): `spin::Mutex` per sezione minima → snapshot,
  rilascio; mai across `.await`.
- **RTC**: letto solo dallo scheduler (BSP) → CMOS port I/O serializzato a un core, no lock.
- **Timer monotono** `timer::ticks()`: atomic globale, multi-core safe in lettura.
- **Esecuzione**: `.cwasm` su compute core (parallelo, via routing esistente), `.wasm` su BSP.
- Nessun nuovo lock condiviso oltre i `Mutex` già presenti.

### 6. Dipendenze / target / attivazione

**Semantica**: `requires:[X]` → start di A tira su X; X fallisce → A `Failed(dep)`.
`after:[X]` → solo ordine (NON tira su X). "X su" = daemon→Running / oneshot→Exited(0).

**Target**: `boot` (durante boot), `post-boot` (dopo shell/desktop su), `manual` (solo CLI/timer).

**Due passate** (`activate_target(t)`, BSP, topo-sort ristretto al set di `t`):
1. `Boot` — nel boot dopo `load_from_disk` + net/storage.
2. `PostBoot` — hook a fine boot, dopo spawn shell/compositor.

Algoritmo per passata:
1. Set = unit `enabled` con quel target + chiusura transitiva dei loro `requires`.
2. Grafo da `after ∪ requires`; **topo-sort (Kahn)**; residuo ⇒ ciclo → unit nel ciclo
   `Failed(cycle)`, log, le altre proseguono.
3. Avvio in ordine; attende "su" prima delle dipendenti (cap timeout es. 10s → log + prosegui).
   `requires` fallito → dipendenti `Failed(dep)`, saltate.

Cross-fase: post-boot dipende da boot → ok (già su). Boot dipende da post-boot → warn, ignorato.
Builtin (ssh, `<builtin>`) partecipano al grafo, marcati Running dal boot phase.

### 7. CLI + host ABI

**`unitctl`** (`.wasm`, wasmi; sostituisce/estende `service`):
`list` · `status <name>` · `start <name>` · `stop <name>` · `enable <name>` ·
`disable <name>` · `timers` · `reload` · `cat <name>`.

**Host fn** (modulo `ruos`, pattern `ruos_service_*` in `wasm/host/`):
```
unit_list(buf, max) -> n            # record fissi: name24 + path40 + status + flags + pid + runs + target
unit_status(name_ptr,len, buf) -> i32
unit_start(name_ptr,len) -> errno   # posta in UNIT_QUEUE (cross-core safe)
unit_stop(name_ptr,len)  -> errno   # request_kill + stop_requested
unit_enable(name_ptr,len, on:i32) -> errno   # registry + riscrive il file (VFS, BSP)
timer_list(buf, max) -> n           # name + unit + schedule-str + next_fire + last_fire
unit_reload() -> errno              # ri-parsa la dir, diff col registry (add/update/remove)
```

**Record blob**: campi fissi NUL-padded (come snapshot service attuale), no serializzazione
complessa nell'ABI.

**Scritture disco**: `enable/disable` → kernel aggiorna registry + riscrive il file
(`.json`/`.yaml`) via VFS sul BSP. Persistente al reboot.

**Errno**: `ServiceError` esteso — NotFound/AlreadyRunning/NotSupported/NoSlot/Parse/Internal.

### 8. Testing

Kernel `no_std`, niente `cargo test` → **boot-checks** (gated `boot-checks`) + manuale.
Funzioni pure (input iniettato) → boot-checkabili.

Boot-checks:
- `parse_yaml`/`parse_json` → `UnitDoc`: campione → asserisce campi.
- `schedule_parse`: `daily 03:00`→Daily{3,0}, `every 300s`→EveryTicks(30000),
  `weekly Mon 09:30`→Weekly{1,9,30}, `hourly :00`→Hourly{0}.
- `compute_next` (critico): now=2026-06-09 14:30 → Daily{3,0}→2026-06-10 03:00;
  Hourly{0}→15:00; rollover giorno/mese/anno; Weekly attraverso la settimana — asserisce epoch.
- `topo_sort`: A after B → [B,A]; requires transitivo; ciclo → rilevato.
- `backoff`: 1s,2s,4s…cap 30s.

Manuale (QEMU/VBox/hardware):
1. `.yaml` + `.json` in `/mnt/etc/units/`, reboot → `unitctl list` (entrambi i formati).
2. oneshot gira una volta; daemon `.cwasm` Running su compute core; kill → restart per policy;
   `unit stop` → resta giù.
3. timer `every 10s` scatta; `daily HH:MM` a orario RTC vicino scatta.
4. deps: A after B → B prima; requires fallito → `Failed(dep)`.
5. `target: post-boot` attiva dopo shell su; `boot` durante boot.
6. `enable/disable` persiste al reboot.

## Fasi di implementazione (un solo progetto)

1. Modello dati (Unit/Timer/enums) + estensione registry.
2. Parser YAML-subset + JSON → `UnitDoc` + builder + `schedule_parse` (+ boot-checks).
3. `compute_next` calendario (+ boot-checks).
4. Supervisor: daemon runner pool + restart/backoff + stop cooperativo.
5. Scheduler task (polling 1s).
6. Dipendenze + topo-sort + `activate_target` (Boot/PostBoot) + hook di boot.
7. `load_from_disk` + persistenza enable/disable.
8. Host ABI + tool `unitctl`.
9. Boot-checks + verifica manuale end-to-end.

## Fuori scope (YAGNI)

- Cron 5-campi (solo sintassi semplificata).
- Backfill di scatti persi a macchina spenta (no `Persistent=`).
- Stop forzato/preemptive (solo cooperativo — coerente con ruos no-preempt).
- Offload daemon `.wasm` su compute core (resta BSP; `.cwasm` per parallelismo).
- Target graph completo systemd (solo 3 fasi: boot/post-boot/manual).
- Socket/path activation, mount units, ecc.
