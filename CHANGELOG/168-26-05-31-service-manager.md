# 168 — service manager (init/systemd-lite)

**Data:** 2026-05-31

## Cosa
Aggiunto un service manager minimale: registry kernel + tool userspace
`service.wasm` + 3 host fn ABI per leggerlo e startare unit.

1. **Kernel — `kernel/src/service/mod.rs`** (nuovo modulo, registrato
   in `kernel/src/main.rs`).
   - `Service { name, path, on_boot, status, pid, runs }` con
     `ServiceStatus::{Idle, Running, Exited(i32), Failed(&'static str)}`.
   - Registry statico `Mutex<Vec<Service>>`. Snapshot owned
     (`ServiceInfo` con `String` per name/path/status) per il bridge
     userspace, così non si tiene il lock durante la format.
   - `ServiceError { NotFound, AlreadyRunning, NotSupported, Internal }`
     con `errno()` di mapping (1/2/3/99) e `Display` (modello
     `SshError`).
   - API: `register(name, path, on_boot)`, `list()`, `status(name)`,
     `start(name)`, `stop(name)` **stub** (ritorna `NotSupported` con
     commento: nessun primitive di cancellazione per fiber wasm
     arbitrarie — vedi nota in modulo), `mark_running/exited/failed`
     usate dal dispatcher.
   - `SERVICE_QUEUE` (single-slot `VecDeque<&'static str>` + waker)
     consumato dal nuovo `service_dispatcher_task` (vedi sotto).
   - `init()` seeda registry a boot: entry `"ssh"` con path
     `"<builtin>"` (SSH è hardcoded-spawned, non passa dal dispatcher)
     + entry `"whoami"` -> `/bin/whoami.wasm` come esempio startable
     per esercitare il dispatcher dalla CLI senza shippare un fixture
     dedicato.

2. **Kernel — `kernel/src/wasm/host/service.rs`** (nuovo).
   - `ruos_service_list(buf, len, used)` — serializza tutta la registry
     come righe TSV `name\tstatus\tpid\truns\tpath\n`. Su buffer
     troppo piccolo ritorna 8 (ENOBUFS) e scrive comunque la size
     richiesta su `used_ptr`.
   - `ruos_service_start(name_ptr, name_len)` — invoca
     `service::start`; ritorna l'errno mappato.
   - `ruos_service_status(name_ptr, name_len, buf, len, used)` — stessa
     serializzazione di list ma per una entry; 1 se NotFound.
   - Registrato in `kernel/src/wasm/host/mod.rs` con la solita
     `service::link(linker)?;`.

3. **Kernel — `kernel/src/executor/mod.rs`**.
   - Nuovo `service_dispatcher_task` (embassy task) sulla falsariga di
     `ssh_pty_dispatcher_task` ([[166-26-05-30-pty-watchdog]]): drena
     `SERVICE_QUEUE`, carica il `.wasm`, instanzia un `Fiber`, registra
     pid in `crate::proc`, chiama `mark_running`, `await fb.run()`,
     poi `unregister` + `mark_exited(name, code)`. In caso di errore
     read/instantiate, `mark_failed(name, "read"|"instantiate")`.
   - Spawn aggiunto in `executor::run` accanto agli altri task.

4. **Kernel — `kernel/src/boot/phases/userland.rs`**.
   - `crate::service::init()` prima di `ssh::spawn()` così la entry
     `"ssh"` esiste quando ci appoggiamo a `mark_running`.
   - Dopo `ssh::spawn()` Ok, `crate::service::mark_running("ssh", 0)`
     (pid sintetico 0 — il task SSH gira come embassy task, non come
     fiber wasm con pid). La CLI mostra l'SSH come `Running`.

5. **Userspace — `user/service/`** (nuovo crate, aggiunto a
   `user/Cargo.toml`).
   - `Cargo.toml` clonato dal pattern di `user/dmesg`.
   - `src/main.rs`: subcommand positional, no flag.
     - `service` / `service list` → tabella allineata
       `NAME STATUS PID RUNS PATH`.
     - `service start <name>` → 0 OK; 1 NotFound/AlreadyRunning/
       NotSupported/altro errno != 0.
     - `service status <name>` → riga summary; 1 se non trovato.
     - `service stop <name>` → stampa "not implemented in MVP" + exit
       3. Subcommand riservato così il futuro ha lo slot.
     - Subcommand ignoto → usage + exit 2.

6. **Wiring**.
   - `Makefile`: `service` aggiunto a `BIN_TOOLS`.
   - `limine.conf`: pair `module_path/module_cmdline` per
     `/bin/service.wasm`.

## Perché
Lo scope è esattamente quanto serve per dire "boot → ssh up → tool che
mostra cosa gira come unit e ne avvia di nuove". È il primo nodo verso
qualcosa stile systemd-lite: registry stabile, dispatcher dedicato,
ABI userspace text-only (split('\t') sufficiente, no decoder binario).

Il `stop` non c'è perché il modello cooperativo non ha cancellation
generica: il flag `proc::request_kill` è best-effort e richiede che
ogni host fn lo controlli — invece di mascherarlo con uno stub
silenzioso, lo lasciamo esplicito come `NotSupported` sia kernel-side
che CLI-side. Il follow-up naturale è: associare un PTY per service
unit (analogo allo shell SSH) e shutdown via SIGHUP-on-EOF
([[166-26-05-30-pty-watchdog]]), ma è una iterazione separata.

## Test
- `make iso` completa senza errori.
- `make run-test` passa la smoke battery (la registry esiste ma
  init.sh smoke non esercita ancora `service` — verifica funzionale
  manuale durante sviluppo, poi rimossa dallo smoke.sh prima del
  commit, come da brief).

## File toccati
- kernel/src/service/mod.rs (nuovo)
- kernel/src/wasm/host/service.rs (nuovo)
- kernel/src/main.rs
- kernel/src/wasm/host/mod.rs
- kernel/src/executor/mod.rs
- kernel/src/boot/phases/userland.rs
- user/service/Cargo.toml (nuovo)
- user/service/src/main.rs (nuovo)
- user/Cargo.toml
- Makefile
- limine.conf
