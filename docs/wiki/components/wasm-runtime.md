# Runtime WASM (wasmi + Wasmtime AOT)

> **Stato:** bozza
> **Aggiornato:** 2026-06-10
> **Fonti:** `kernel/src/wasm/`, `kernel/src/wasm/host/`, `kernel/src/wasm/wt/`
> **Spec collegate:** `docs/superpowers/specs/2026-05-28-rust-wasix-bootstrap-design.md`,
> `docs/superpowers/specs/2026-04-04-egui-desktop-wasmtime-aot-design.md`

## Cos'è

Il runtime WASM è il cuore dello userland di ruOS: **tutto ciò che non è il kernel
è un modulo WebAssembly**. Coesistono due runtime, scelti dal router della shell
in base all'estensione del file:

- **wasmi** — interprete puro Rust `no_std`. Esegue i tool CLI `.wasm`
  (`wasm32-wasip1`, con `std`). Sicuro ma lento.
- **Wasmtime AOT** — runtime `no_std`, senza JIT. Esegue i `.cwasm` precompilati
  (GUI, compositor, componenti WIT) a velocità quasi-nativa. I moduli sono
  compilati AOT sull'host con `tools/wt-precompile` e `include_bytes!`'d o
  caricati da `/bin`.

## Dove vive

| File / cartella | Ruolo |
|-----------------|-------|
| `kernel/src/wasm/mod.rs` | Entry point, router `.wasm`→wasmi / `.cwasm`→Wasmtime |
| `kernel/src/wasm/host/` | Host fn wasmi: `mod.rs` (linker), `fd.rs`, `path.rs`, `lifecycle.rs`, `clock.rs`, `random.rs`, `proc.rs`, `sysinfo.rs`, `sock.rs`, `term.rs`, `service.rs`, `smp.rs`, `mem.rs` |
| `kernel/src/wasm/fiber.rs` | Fiber per task (sospensione cooperativa) |
| `kernel/src/wasm/suspend.rs` | `SuspendReason` enum |
| `kernel/src/wasm/exec_queue.rs` | Coda di esecuzione tool |
| `kernel/src/wasm/pipeline.rs` | Esecuzione pipeline `a | b | c` con pipe in-RAM |
| `kernel/src/wasm/wt/` | Wasmtime: `mod.rs` (runtime), `gfx.rs` (ruos_gfx), `wm.rs` (compositor), `wasi.rs` (WASI subset), `sys.rs` (telemetria), `term.rs` (PTY bridge), `component.rs` (WIT) |
| `kernel/src/wasm/host/mem.rs` | Unico accessor guest memory (bounds-checked, fuzz-tested) |

## Modello: due runtime, un'ABI

```
           .wasm (CLI tool)                  .cwasm (GUI / component)
                │                                     │
       ┌────────▼────────┐               ┌───────────▼────────────┐
       │     wasmi        │               │    Wasmtime AOT        │
       │  (interprete)    │               │  (no JIT, no_std)      │
       ├──────────────────┤               ├────────────────────────┤
       │ wasi_snapshot_   │               │ wasi_snapshot_preview1 │
       │   preview1 (25)  │               │   (subset per std)     │
       │ ruos (33)        │               │ wm (20) + sys (4)      │
       │                  │               │ term (5) + ruos_gfx (6)│
       └────────┬─────────┘               │ WIT components         │
                │                         └───────────┬────────────┘
                └─────────────┬───────────────────────┘
                              │
                     Kernel services
                (VFS, PTY, net, gfx, proc…)
```

### Host ABI

Ogni runtime espone al guest un set di **host functions** organizzate per modulo
di import:

| Modulo | Runtime | Funzioni | Per cosa |
|--------|---------|----------|----------|
| `wasi_snapshot_preview1` | wasmi | 25 | WASI P1: file, args, env, clock, random |
| `ruos` | wasmi | 33 | Custom: exec, readdir, net, disk, sysinfo |
| `wm` | Wasmtime | 20 | Window manager: commit, poll_event, spawn |
| `sys` | Wasmtime | 4 | Telemetria: cpustat, proc_stat, meminfo |
| `term` | Wasmtime | 5 | Terminal bridge: open/read/write/resize/close |
| `ruos_gfx` | Wasmtime | 6 | Raw framebuffer: blit, poll, gfx_info |
| `ruos:gui/*`, `ruos:tui/*` (WIT) | Wasmtime | varies | Typed component model bridge (inclusi TUI component linking) |

Documentazione dettagliata in [`docs/api/`](../../api/README.md).

## Comportamento a runtime

### Fiber e sospensione cooperativa

Ogni task WASM gira su una **fiber** (`wasm/fiber.rs`). Quando un host call
blocca (es. `fd_read` su un PTY senza input, `tcp_dial`, `ping`, `readdir`), la
fiber **sospende** con un `SuspendReason`. L'executor async esegue altro lavoro
(timer, socket, altri task). Quando l'evento atteso arriva (keystroke, timer,
risposta TCP), la fiber **riprende** esattamente dove si era fermata. Dal punto
di vista del guest, il call ha semplicemente bloccato.

### Fuel metering e Epoch Watchdog

Per evitare che task malfunzionanti (o in loop infinito) monopolizzino la CPU:

- **wasmi (fuel metering)**: Ogni slice di esecuzione ha un budget di **2.000.000.000 istruzioni** (`FUEL_PER_SLICE`). Un loop CPU-bound senza host call esaurisce il fuel ed è **killed** (exit 137). I task I/O-bound ricaricano il fuel ad ogni host call e girano indefinitamente.
- **Wasmtime (epoch-based scheduling)**: Il motore Wasmtime utilizza l'opzione `epoch_interruption(true)`. Il timer IRQ a 100 Hz incrementa globalmente un epoch clock. I task Wasmtime specificano una `deadline` e se questa scade durante l'esecuzione pura su CPU, il task è trappato come se fosse andato in timeout (watchdog timeout) o sospeso per far girare altri task.

### Resource limits

Un `wasmi::ResourceLimiter` limita le **pagine di linear memory** e gli **elementi
di table** per istanza. Un guest non può allocare memoria illimitata.

### Capability-scoped paths

Le funzioni host che accedono al filesystem rifiutano path che **escono dalla root
dichiarata** del task: nessun `../` oltre `/`. La sandbox è path-based.

### Guest memory accessor

- **wasmi**: **Tutti** gli accessi alla linear memory del guest passano per
  `wasm/host/mem.rs::check_bounds` — un unico punto auditato e fuzz-tested (casi
  avversariali: ptr negativo, len overflow, boundary). Nessun host fn legge/scrive
  raw nella guest memory bypassando questo check.
- **Wasmtime**: I moduli AOT beneficiano di memory isolation tramite demand paging, 
  riservando uno spazio VA configurato (`memory_reservation`, `memory_guard_size`).
  La memoria in eccesso usa guard pages configurate dal runtime, mitigando 
  l'overflow e gestendo la sicurezza a livello architetturale ed AOT.

## Router della shell

Il comando `exec` della shell (e il suo equivalente SSH) decide il runtime:
- Estensione `.cwasm` → **Wasmtime** (via `wasm/wt/mod.rs`)
- Estensione `.wasm` → **wasmi** (via `wasm/exec_queue.rs`)

Le pipeline `a | b` passano per `wasm/pipeline.rs`, che avvia le fiber come
stadi concorrenti collegati da pipe in-RAM (`kernel/src/pipe/`).

## Vincoli e limiti

- **wasmi è lento**: l'interprete è ~100× più lento del codice nativo. Per le GUI
  è inutilizzabile → Wasmtime AOT.
- **Wasmtime è no_std, runtime-only**: non include Cranelift, non può compilare a
  runtime. I `.cwasm` devono essere precompilati sull'host.
- **Fuel non è preemption**: in `wasmi`, un guest che non fa host call e non esaurisce il fuel
  in un tick monopolizza la CPU per quel tick (10 ms a 100 Hz). In `Wasmtime`, questo è mitigato
  dall'epoch watchdog.
- **Single-address-space**: la sandbox è il runtime WASM, non la MMU o le page table per processo. 
  Un bug nel runtime stesso (o nell'`unsafe` del kernel) è fatale per tutto il sistema.

## Insidie / note

- `FUEL_PER_SLICE` è calibrato per non essere troppo basso (ucciderebbe un guest
  che fa poche host call ma molto calcolo legittimo) né troppo alto (un guest
  runaway occuperebbe la CPU troppo a lungo).
- `check_bounds` va usato **sempre**: aggiungere un host fn che legge guest memory
  senza passarci è un bug di sicurezza.
- Il router `.cwasm` di Wasmtime richiede `cld` prima di ogni invocazione del guest
  (la SysV ABI assume DF=0; il codice Cranelift usa `rep movs`).

## Vedi anche

- [API reference](../../api/README.md) — documentazione per ogni host function
- [Compositor](compositor.md) — il compositor che gira su Wasmtime
- [Architettura — panoramica](../architecture/overview.md)
- [Indice della wiki](../README.md)
