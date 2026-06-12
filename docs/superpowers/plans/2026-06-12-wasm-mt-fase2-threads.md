# WASM MT Fase 2 — wasm-threads MVP (fiber M:N) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `std::thread` + rayon funzionanti nelle app `.cwasm` (`wasm32-wasip1-threads`): thread = fiber cooperativi M:N sui core ComputeApp, `atomic.wait` sospende il fiber, `atomic.notify` = IPI.

**Architecture:** Fork minimale vendorizzato di wasmtime 45 (feature `threads` senza `std`: i libcall `memory_atomic_wait32/64/notify` deviati su hook `extern "C"` futex che il kernel implementa; `SharedMemory` su sync interno no_std). I thread (e il `_start` dei moduli threaded) girano dentro fiber con stack-switch reale (`wasmtime-internal-fiber`, backend no_std). Scheduler in `kernel/src/wasm/wt/threads.rs`: run-queue + wait-queue futex sharded, drenata da `run_core()` accanto al pool (stessi wake-source/IPI). La linear memory dei moduli threaded è IMPORTATA (`env::memory`, shared) — l'host crea la `SharedMemory` e la definisce nel Linker prima dell'instantiate.

**Tech Stack:** wasmtime 45 forkato (path-patch), `wasmtime-internal-fiber =45.0.0` (no_std), target guest `wasm32-wasip1-threads`, infra Fase 1 (pool/IPI/epoch/demand-paging). Build/test via WSL `Ubuntu-22.04`, repo `/mnt/w/Work/GitHub/ruos`. QEMU `-smp 4 -m 2048`; VBox per i cambi CPU-sensitive.

---

## Premesse verificate (ricerca 2026-06-12 — NON re-investigare)

**Wasmtime/kernel:**
- Dep: `kernel/Cargo.toml:24` `wasmtime = "=45.0.0"`, default-features=false, features `["runtime","custom-virtual-memory","custom-sync-primitives","component-model"]`. NON vendorizzato (crates.io). Lock: `kernel/Cargo.lock`.
- **`threads` feature upstream = `["wasmtime-cranelift?/threads","wasmtime-winch?/threads","std"]` → hard-std.** `parking_spot.rs` usa `std::thread::park_timeout`/`Thread::unpark`; `shared_memory.rs` usa `std::sync::RwLock` + `std::time::Instant`. Senza feature il kernel compila `shared_memory_disabled`. `Config::wasm_threads` è `#[cfg(feature="threads")]`. Fonte registry WSL: `/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wasmtime-45.0.0/`.
- I hook `custom-sync-primitives` esistenti (`platform.rs:34-123`: `wasmtime_tls_get/set`, `wasmtime_sync_lock_*`, `wasmtime_sync_rwlock_*`) coprono lock/rwlock interni, NON il parking.
- **`wasmtime-internal-fiber` 45 ha backend no_std** (`src/lib.rs` cfg_if → `mod nostd` + `mod stackswitch` asm; stack in `TryVec<u8>`, no guard pages). Solo feature `std=[]`. Assente da `kernel/Cargo.lock` (presente solo in wt-precompile, host).
- Engine kernel: `wt/mod.rs:182-187` (`ENGINE: spin::Once`), config `engine_config()` `wt/mod.rs:289-319`: `signals_based_traps(false)`, `memory_init_cow(false)`, `epoch_interruption(true)` (hashato!), `memory_reservation(256<<20)`, `memory_guard_size(0)`, `memory_reservation_for_growth(64<<20)` (runtime-only), `memory_may_move(true)`, `x86_float_abi_ok(true)`, ISA fissa sse3/ssse3/sse4.1/sse4.2. wt-precompile speculare (`tools/wt-precompile/src/main.rs:32-64`), CLI `wt-precompile [--component] <in> <out>`, accetta `.wat` (feature `wat`).
- Epoch: timer BSP 100 Hz → `epoch_tick()` (`wt/mod.rs:192-196`). CLI tools usano `NO_DEADLINE_TICKS = u64::MAX/2` (`wt/mod.rs:180`).

**Exec path .cwasm CLI:**
- shell → `ruos.exec` → `SuspendReason::Exec` → router `fiber.rs:584-610`: `.cwasm` non-compositor → `exec_cwasm_parallel` → `pick_compute_core()` + `spawn_on(core, run_app_on_core(...))` (`executor/mod.rs:127-138`) → **`run_cwasm` SINCRONO sull'AP** (`wt/mod.rs:208-279`). 1-2 core → inline. Exit via `ExecReply`/`ExecReplyFuture`.
- `run_cwasm`: deserialize (`:223`) → `WtState::new(args)` + `stdout_pty` (`:227-235`) → `set_epoch_deadline(NO_DEADLINE_TICKS)` → linker = `wasi::add_to_linker` + `gfx` + `gui` (`:239-251`) → `instantiate` → `cld` → `_start` typed `(),()` (`:252-272`). **Nessun `store.limiter`** su questo path.
- `WtState` (`wt/state.rs:14-23`): `args: Vec<Vec<u8>>`, `exit`, `fds: Vec<WtFd>` (0/1/2 console, 3 preopen "/"), `stdout_pty`. **NIENTE env**; `environ_sizes_get/get` stub 0 (`wasi.rs:255-262`).
- Surface WASI wt (`wasi.rs`, modulo `wasi_snapshot_preview1`, generico `T: HasWasi`): proc_exit, fd_write, fd_read, fd_seek, fd_close, fd_fdstat_get, fd_filestat_get, path_filestat_get, fd_prestat_get, fd_prestat_dir_name, path_open, args_*, environ_* (stub), clock_time_get, random_get, sched_yield. **Manca tutto il resto.**
- Accessor memoria: `mem.rs:8-13` `caller.get_export("memory")` — funziona coi moduli threads perché wasm-ld usa `--import-memory --export-memory` (re-export "memory").

**ABI wasi-threads (verificata):**
- Import guest: modulo **`"wasi"`**, field **`"thread-spawn"`** (TRATTINO), `(param i32) (result i32)` — param = `start_arg` ptr; ritorno tid>0 o errno negativo.
- Export da chiamare sulla NUOVA instance: **`wasi_thread_start(tid: i32, start_arg: i32)`**, nessun result.
- Target spec rustc (verificato su nightly pinnato): link flags `--import-memory --export-memory --shared-memory --max-memory=1073741824`, `-z stack-size=1048576`, features `+atomics,+bulk-memory,+mutable-globals`. → il modulo importa `(import "env" "memory" (memory N MAX shared))`.
- Data segments passivi + `__wasm_init_memory` one-shot atomico → re-instanziare lo stesso modulo NON ri-clobbera la memoria. TLS/stack del thread: TUTTO guest-side (`wasi_thread_start` shim setta `__stack_pointer` e `__tls_base` dal blocco `start_arg` preparato da `pthread_create` nella memoria condivisa). L'host: stesso Module + stessa SharedMemory + fresh Instance + chiamare l'export. Architettura upstream wasmtime-wasi-threads: 1 Engine + 1 Module + 1 SharedMemory; per thread = 1 Store + 1 Instance.
- `std::thread::available_parallelism()` su wasi = `Err(Unsupported)`; rayon legge **`RAYON_NUM_THREADS`** dall'environ, altrimenti 1 thread. → serve env injection.

**Executor/SMP:**
- `run_core` (`executor/mod.rs:272-369`): poll → `drain_inbox` → drain pool (`while let Some(slot) = pool::take()`) → check wake-sources sotto `interrupts::disable()` (`WAKE_PENDING || inbox::is_pending || !pool::is_empty()`) → `enable_and_hlt()`. **Seam = quella disgiunzione + i producer che fanno wake** (pool::submit broadcast `send_ipi_all_but_self(VEC_WAKE)` pool.rs:63; inbox targeted `send_ipi(lapic_id_of(t), VEC_INBOX)`).
- Ruoli: core0=BspIo, core1=GuiCompositor (`gui_worker_loop`, NON esegue run_core né drena pool), core2+=ComputeApp (`smp/mod.rs:25,52-59`; `ap.rs:82-89`). `pick_compute_core()` round-robin; `first_compute_app_core()`. AP stack = 8 MiB heap leaked (`ap.rs:62-73`).
- IPI targeted: `lapic::send_ipi(lapic_id, vector)` (`lapic.rs:87-99`) + `cpu::lapic_id_of`. `wake_core(owner)` (`executor/mod.rs:1130-1135`).
- **Fiber wasmi esistente = ResumableCall interpreter-state, ZERO stack-switching, NON riusabile.** Unico asm rsp nel kernel = bootstrap AP one-way. Wasmtime gira sync sullo stack del core.

**Bring-up gate precedente:** `bringup.cwasm` (Makefile:211-219, wasm-tools component new + wt-precompile --component), embedded `#[cfg(feature="boot-checks")]` (`wt/mod.rs:68-69`), eseguito in `boot/phases/interrupts.rs:207-209`, marker `[component] WT-COMPONENT-OK`, assert in `test-boot` (Makefile:589). I gate WT girano nella fase `interrupts` DOPO `smp::bringup()` e timer (engine lazy `spin::Once`). Stile boot-check: `binfo!` una riga ok/FAIL, log-and-continue.
- Embedded guest nuovo: regola Makefile in `WT_KCWASMS` (Makefile:124-142) + `include_bytes!` gated in `wt/mod.rs` + runner `run_*_demo()` + chiamata in `interrupts.rs`.
- Compat `.cwasm`: tunables hashate; abilitare `wasm_threads` aggiunge una feature ai moduli NUOVI — i `.cwasm` esistenti devono continuare a deserializzare (verifica nel Task 1).

## Decisioni architetturali del piano (derivate dalla ricerca, coerenti con la spec)

1. **Niente `async_support`.** Wasmtime resta sync; ogni esecuzione guest threaded (main `_start` incluso) gira dentro un fiber NOSTRO (`wasmtime-internal-fiber`, backend no_std). L'hook futex sospende il fiber. Soddisfa il punto (3) del gate ("atomic.wait cede il core") con meno superficie del previsto.
2. **Fork = vendor del solo crate `wasmtime`** in `third_party/wasmtime45/` (copiato dal registry, stesso nome+versione, `[patch.crates-io]` nel kernel). Diff minimale: (a) feature `threads` senza `"std"`; (b) `shared_memory.rs` su sync interno no_std; (c) libcall `memory_atomic_{wait32,wait64,notify}` → hook `extern "C"` futex. wt-precompile resta su crates.io STOCK + feature `threads` (host std). Stessa versione ⇒ compat hash ok.
3. **Il main dei moduli threaded gira su fiber**: `run_cwasm` rileva l'import di memoria shared → route a `threads::exec_threaded()`. Altrimenti il futex hook non avrebbe un fiber da sospendere quando il MAIN fa `pthread_join`.
4. **Deadline epoch dei thread store = `NO_DEADLINE_TICKS`** (stesso precedente dei CLI tools, `wt/mod.rs:236-238`): una deadline assoluta trapperebbe al resume dopo un park lungo. La protezione del desktop resta strutturale (un thread runaway occupa UN core ComputeApp; GUI su core 1 intoccata). Deviazione documentata dalla spec §7 (epoch per-thread) — il kill-the-group su trap resta.
5. **TLS swap obbligatorio**: il TLS wasmtime (`platform.rs`, per-core) va salvato/ripristinato a ogni suspend/resume del fiber (l'activation chain di una call sospesa vive nello stack del fiber; il puntatore va con lui). Permette anche la migrazione cross-core.
6. **Env injection**: `WtState.env` nuovo + `environ_sizes_get/get` reali + `RAYON_NUM_THREADS=<n_compute>` iniettato da `exec_threaded`.

## File Structure

- **Create** `third_party/wasmtime45/` — vendor del crate wasmtime 45.0.0 + patch (3 file: `Cargo.toml`, `src/runtime/vm/memory/shared_memory.rs`, `src/runtime/vm/libcalls.rs`; eventuali cfg in `src/runtime/vm/memory.rs`/`vm.rs`).
- **Create** `kernel/src/wasm/wt/threads.rs` — TUTTO lo scheduler: `ThreadGroup`, slab fiber, run-queue, wait-queue futex sharded `#[repr(align(64))]`, `exec_threaded`, `spawn_thread`, hook `wasmtime_futex_*`, `take_runnable`/`run_one`/`runnable_empty`, TLS swap, kill-group, marker.
- **Modify** `kernel/Cargo.toml` — `[patch.crates-io]`, feature `threads`, dep `wasmtime-internal-fiber`.
- **Modify** `kernel/src/wasm/wt/mod.rs` — `engine_config(): wasm_threads(true)`, route threaded in `run_cwasm`, `mod threads`, include gate guests.
- **Modify** `kernel/src/wasm/wt/state.rs` — `WtState.env` + `threads: Option<Arc<ThreadGroup>>`.
- **Modify** `kernel/src/wasm/wt/wasi.rs` — environ reali; registrazione `("wasi","thread-spawn")`.
- **Modify** `kernel/src/executor/mod.rs` — seam run_core (drain fiber + wake-source).
- **Modify** `tools/wt-precompile/` — feature `threads` + `config.wasm_threads(true)`.
- **Create** `tools/wt-threads-gate/{gate1.wat,gate2.wat,gate3.wat}` — guest dei 3 gate.
- **Modify** `kernel/src/boot/phases/interrupts.rs` — runner gate boot-checks.
- **Create** `tools/parsum/` (rayon, wasm32-wasip1-threads) + `tools/mtstress/` (mutex conteso) — app test.
- **Create** `tests/threads-test.sh`; **Modify** `Makefile` (regole .cwasm + test target), `build-iso.ps1` (target rustup), `docs/api/wasi.md` (o pagina equivalente: import thread-spawn + environ).
- **Create** `CHANGELOG/479-...` (e successivi per-commit).

---

### Task 0: Toolchain + vendor fork + build verde (gate hard di compilazione)

**Files:**
- Create: `third_party/wasmtime45/` (vendor + patch)
- Modify: `kernel/Cargo.toml`, `tools/wt-precompile/Cargo.toml`, `tools/wt-precompile/src/main.rs`

- [ ] **Step 1: Target rust + vendor**

```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'source ~/.cargo/env && rustup target add wasm32-wasip1-threads'
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && cp -r /root/.cargo/registry/src/index.crates.io-*/wasmtime-45.0.0 third_party/wasmtime45 && rm -f third_party/wasmtime45/.cargo-checksum.json && ls third_party/wasmtime45/src/runtime/vm/memory/shared_memory.rs'
```
Expected: il file esiste. (Il crate registry è self-contained: nessun riferimento workspace.)

- [ ] **Step 2: Patch feature `threads` senza std**

In `third_party/wasmtime45/Cargo.toml`, trova la riga della feature `threads` e rimuovi `"std"`:
```toml
threads = ["wasmtime-cranelift?/threads", "wasmtime-winch?/threads"]
```

- [ ] **Step 3: `[patch.crates-io]` + feature nel kernel**

In `kernel/Cargo.toml`: alla dep wasmtime aggiungi `"threads"` alle features, e in fondo al file:
```toml
[patch.crates-io]
# Fork minimale wasmtime 45 per threads in no_std (MT Fase 2):
# threads senza "std", SharedMemory su sync interno, atomic wait/notify
# deviati sugli hook extern "C" wasmtime_futex_* (implementati in wt/threads.rs).
# Spec: docs/superpowers/specs/2026-06-12-wasm-mt-fase2-threads-design.md
wasmtime = { path = "../third_party/wasmtime45" }
```

- [ ] **Step 4: Loop compile-driven sul fork (il grosso del task)**

```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos/kernel && source ~/.cargo/env && cargo check --target x86_64-unknown-none -Zbuild-std=core,alloc 2>&1 | head -60'
```
(Usa il comando di check che il Makefile usa per il kernel se diverso — guarda `make build`.)
Expected: errori SOLO dentro `third_party/wasmtime45` nei moduli threads-gated. Fixali così, ricompilando a ogni passo:

a) **`src/runtime/vm/memory/shared_memory.rs`** — sostituisci gli import std col sync interno del crate: cerca nel vendor il modulo sync no_std già usato altrove (`grep -rn "custom-sync-primitives\|mod sync" third_party/wasmtime45/src | head`) e usa quegli stessi tipi (`Mutex`/`RwLock` interni → finiscono sui nostri hook `wasmtime_sync_*` di platform.rs). `std::time::Instant`: usato solo per i timeout di wait — quel path viene bypassato al punto (b); rimuovi/cfg-gata i resti. `std::sync::Arc` → `alloc::sync::Arc`.

b) **`src/runtime/vm/libcalls.rs`** — trova `memory_atomic_notify`, `memory_atomic_wait32`, `memory_atomic_wait64`. MANTIENI la validazione/bounds-check upstream; sostituisci SOLO la parte parking (`parking_spot`) con:
```rust
unsafe extern "C" {
    fn wasmtime_futex_wait32(addr: *const u32, expected: u32, timeout_ns: i64) -> u32;
    fn wasmtime_futex_wait64(addr: *const u64, expected: u64, timeout_ns: i64) -> u32;
    fn wasmtime_futex_notify(addr: *const u8, count: u32) -> u32;
}
```
Contratto di ritorno wait (semantica wasm threads): 0=woken, 1=not-equal, 2=timed-out. `timeout_ns < 0` = infinito. notify ritorna il numero di waiter svegliati.

c) **`src/runtime/vm.rs` / `memory.rs`** — se `parking_spot` resta referenziato sotto cfg threads, cfg-gatalo via o sostituiscilo con un modulo stub che rimanda agli extern sopra.

Itera finché `cargo check` del kernel è PULITO. Regola: il diff del fork resta MINIMO e ogni hunk commentato con `// ruos:` per i futuri rebase.

- [ ] **Step 5: wt-precompile con threads**

`tools/wt-precompile/Cargo.toml`: aggiungi `"threads"` alle features wasmtime (stock crates.io, NON il fork). `tools/wt-precompile/src/main.rs` dopo `config.epoch_interruption(true);`:
```rust
config.wasm_threads(true); // proposta threads: atomics nativi + shared memory (MT Fase 2)
```

- [ ] **Step 6: Engine kernel con threads**

`kernel/src/wasm/wt/mod.rs`, in `engine_config()` accanto a `epoch_interruption`:
```rust
config.wasm_threads(true); // deve combaciare con wt-precompile (regola deserialize)
```

- [ ] **Step 7: Regressione compat — i .cwasm esistenti devono ancora caricare**

```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && make run-test'
```
Expected: `TEST_PASS`. Se i deserialize falliscono per mismatch features → STOP: investigare se `wasm_threads` entra nell'hash; nel caso, re-AOT di tutto via `make iso` è automatico per i system app; annotare in changelog che `apps/` drop-folder e `/mnt/apps` vanno ri-precompilati (precedente: changelog 455).

- [ ] **Step 8: Commit**

```bash
git add third_party/wasmtime45 kernel/Cargo.toml kernel/Cargo.lock tools/wt-precompile docs/… 
git commit -m "feat(wt): vendored wasmtime fork — threads feature in no_std (futex hooks, nostd SharedMemory)"
```
(+ changelog `CHANGELOG/NNN-26-06-12-wasmtime-fork-threads-nostd.md` nello stesso commit, formato standard — NNN = il numero più alto in `CHANGELOG/` + 1, ricontrolla: fino a 484 già preso.)

---

### Task 1: Gate 1 — atomics + SharedMemory nel kernel (THREADS-OK 1)

**Files:**
- Create: `tools/wt-threads-gate/gate1.wat`
- Modify: `Makefile` (regola + `WT_KCWASMS`), `kernel/src/wasm/wt/mod.rs` (include + runner), `kernel/src/boot/phases/interrupts.rs`

- [ ] **Step 1: Guest gate1**

`tools/wt-threads-gate/gate1.wat` — modulo CORE con memoria shared IMPORTATA che fa un RMW atomico e ritorna il valore:
```wat
(module
  (import "env" "memory" (memory 1 1 shared))
  (func (export "run") (result i32)
    ;; old = atomic_add(mem[16], 41) ; poi rileggi e ritorna mem[16] (= 41+1)
    (drop (i32.atomic.rmw.add (i32.const 16) (i32.const 41)))
    (drop (i32.atomic.rmw.add (i32.const 16) (i32.const 1)))
    (i32.atomic.load (i32.const 16))))
```

- [ ] **Step 2: Regola Makefile**

Accanto alle regole hello/spin (Makefile:124-142):
```make
$(WT_KDIR)/threads_gate1.cwasm: tools/wt-threads-gate/gate1.wat $(WT_PRECOMPILE)
	$(WT_PRECOMPILE) $< $@
```
e aggiungi `$(WT_KDIR)/threads_gate1.cwasm` a `WT_KCWASMS`.

- [ ] **Step 3: Runner kernel**

`kernel/src/wasm/wt/mod.rs`, pattern di `run_hello_demo` (gated boot-checks):
```rust
#[cfg(feature = "boot-checks")]
static THREADS_GATE1_CWASM: &[u8] = include_bytes!("threads_gate1.cwasm");

/// Gate 1 MT Fase 2: SharedMemory + atomics nativi nell'engine no_std.
/// Crea la SharedMemory host-side, la importa, esegue un RMW atomico.
#[cfg(feature = "boot-checks")]
pub fn run_threads_gate1() -> bool {
    let engine = engine();
    let module = match unsafe { wasmtime::Module::deserialize(engine, THREADS_GATE1_CWASM) } {
        Ok(m) => m, Err(e) => { crate::kprintln!("ruos: gate1 deserialize: {:?}", e); return false; }
    };
    // Tipo memoria dall'import del modulo (shared, min=max=1).
    let mem_ty = match module.imports().find_map(|i| i.ty().memory().cloned()) {
        Some(t) => t, None => { crate::kprintln!("ruos: gate1: no memory import"); return false; }
    };
    let shared = match wasmtime::SharedMemory::new(engine, mem_ty) {
        Ok(s) => s, Err(e) => { crate::kprintln!("ruos: gate1 SharedMemory: {:?}", e); return false; }
    };
    let mut store = wasmtime::Store::new(engine, ());
    store.set_epoch_deadline(NO_DEADLINE_TICKS);
    let mut linker: wasmtime::Linker<()> = wasmtime::Linker::new(engine);
    if linker.define(&store, "env", "memory", shared.clone()).is_err() { return false; }
    let inst = match linker.instantiate(&mut store, &module) {
        Ok(i) => i, Err(e) => { crate::kprintln!("ruos: gate1 instantiate: {:?}", e); return false; }
    };
    let run = match inst.get_typed_func::<(), i32>(&mut store, "run") {
        Ok(f) => f, Err(_) => return false,
    };
    match run.call(&mut store, ()) {
        Ok(42) => true,
        other => { crate::kprintln!("ruos: gate1 run = {:?} (want Ok(42))", other); false }
    }
}
```
(Adatta i nomi API reali del fork se differiscono — `SharedMemory::new(engine, MemoryType)` è l'API wasmtime 45; il deserialize di un modulo con shared memory ora deve passare perché `wasm_threads(true)`.)

- [ ] **Step 4: Chiamata in boot phase**

`kernel/src/boot/phases/interrupts.rs`, nel blocco boot-checks accanto a `run_bringup_demo` (riga ~207):
```rust
// MT Fase 2 gate 1: SharedMemory + atomics nativi (fork wasmtime no_std).
let g1 = crate::wasm::wt::run_threads_gate1();
crate::binfo!("wt", "THREADS-OK 1 = {}", if g1 { "ok" } else { "FAIL" });
```

- [ ] **Step 5: Build + boot, assert marker**

```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && make iso CARGO_FEATURES=boot-checks && timeout 60 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -m 2048 -cdrom build/os.iso -serial file:build/g1.log -display none -no-reboot -device qemu-xhci >/dev/null 2>&1; grep -a "THREADS-OK 1" build/g1.log'
```
Expected: `THREADS-OK 1 = ok`. Se FAIL → debug del fork (è il punto previsto dal gate; NON proseguire).

- [ ] **Step 6: Commit** (`feat(wt): threads gate 1 — shared memory + native atomics in no_std`).

---

### Task 2: Fiber runtime (threads.rs nucleo) + self-test host-only

**Files:**
- Create: `kernel/src/wasm/wt/threads.rs`
- Modify: `kernel/Cargo.toml` (dep fiber), `kernel/src/wasm/wt/mod.rs` (`pub mod threads;`), `kernel/src/executor/mod.rs` (seam), `kernel/src/boot/phases/interrupts.rs` (self-test)

- [ ] **Step 1: Dep fiber**

`kernel/Cargo.toml`:
```toml
# Stack-switch fiber per i wasm-thread (MT Fase 2). Backend no_std upstream.
wasmtime-internal-fiber = { version = "=45.0.0", default-features = false }
```
Verifica API reale: `wsl … cargo doc` o leggi `/root/.cargo/registry/src/…/wasmtime-internal-fiber-45.0.0/src/lib.rs` — attesi `FiberStack::new(size)`, `Fiber::new(stack, closure(Resume, &mut Suspend))`, `fiber.resume(val)`, `suspend.suspend(val)`. Adatta i nomi esatti.

- [ ] **Step 2: Scheletro scheduler in `threads.rs`**

```rust
//! MT Fase 2 — scheduler dei wasm-thread come fiber cooperativi M:N.
//! Spec: docs/superpowers/specs/2026-06-12-wasm-mt-fase2-threads-design.md
//! I fiber girano sui core ComputeApp dentro run_core(); cedono SOLO a
//! atomic.wait (hook futex), host-call bloccante o return. TLS wasmtime
//! swappato a ogni suspend/resume (l'activation vive nello stack del fiber).

use alloc::{boxed::Box, sync::Arc, vec::Vec, collections::VecDeque};
use core::sync::atomic::{AtomicU32, AtomicUsize, AtomicBool, Ordering};
use crate::sync::IrqMutex;

pub const FIBER_STACK_SIZE: usize = 2 * 1024 * 1024; // frame nativi cranelift+host; max_wasm_stack=512K
const WAITQ_SHARDS: usize = 16;

/// Un fiber-thread registrato. Lo stato di esecuzione vive nello stack del fiber.
struct ThreadFiber {
    fiber: wasmtime_internal_fiber::Fiber<'static, (), (), i32>, // resume=(), yield=(), return=exit code
    /// TLS wasmtime salvato mentre il fiber è sospeso (vedi run_one).
    saved_tls: *mut u8,
    /// Suspend handle pubblicato dal corpo del fiber (per l'hook futex).
    suspend_ptr: AtomicUsize,
    group: Arc<ThreadGroup>,
    tid: u32,
}
unsafe impl Send for ThreadFiber {}

/// Stato condiviso di UNA app threaded (1 Module + 1 SharedMemory + N thread).
pub struct ThreadGroup {
    pub module: wasmtime::Module,
    pub linker: Arc<wasmtime::Linker<crate::wasm::wt::state::WtState>>,
    pub shared: wasmtime::SharedMemory,
    pub next_tid: AtomicU32,
    pub live: AtomicU32,
    pub poisoned: AtomicBool,           // trap in un thread → muore il gruppo
    pub exit: IrqMutex<Option<i32>>,    // exit code del main
    pub base_args: Vec<Vec<u8>>,
    pub env: Vec<Vec<u8>>,
}

/// Run-queue globale dei fiber runnable (per-core sharding = ottimizzazione futura;
/// la contesa qui è bassa: enqueue solo a spawn/notify).
static RUNQ: IrqMutex<VecDeque<Box<ThreadFiber>>> = IrqMutex::new(VecDeque::new());

/// Fiber in attesa futex: shard per indirizzo. align(64) anti false-sharing.
#[repr(align(64))]
struct WaitShard(IrqMutex<Vec<(usize /*addr*/, u64 /*deadline tick, MAX=inf*/, Box<ThreadFiber>)>>);
static WAITQ: [WaitShard; WAITQ_SHARDS] = [const { WaitShard(IrqMutex::new(Vec::new())) }; WAITQ_SHARDS];
#[inline] fn shard(addr: usize) -> &'static WaitShard { &WAITQ[(addr >> 3) % WAITQ_SHARDS] }

/// Fiber correntemente in esecuzione su ogni core (per l'hook futex).
static CURRENT: [AtomicUsize; crate::cpu::MAX_CPUS] =
    [const { AtomicUsize::new(0) }; crate::cpu::MAX_CPUS];

pub fn runnable_empty() -> bool { RUNQ.lock().is_empty() }

fn enqueue_runnable(f: Box<ThreadFiber>) {
    RUNQ.lock().push_back(f);
    // Sveglia un core dormiente (stesso pattern di pool::submit).
    crate::apic::lapic::send_ipi_all_but_self(crate::idt::VEC_WAKE);
}

/// Drena ed esegue UN fiber runnable. Chiamato da run_core sui core abilitati.
/// Ritorna true se ha eseguito qualcosa.
pub fn run_one(cpu: u32) -> bool {
    let mut f = match RUNQ.lock().pop_front() { Some(f) => f, None => return false };
    // TLS swap: dentro va il TLS del fiber, fuori si ripristina quello del core.
    let prev = crate::wasm::wt::platform::tls_raw_get();
    crate::wasm::wt::platform::tls_raw_set(f.saved_tls);
    CURRENT[cpu as usize].store(&mut *f as *mut ThreadFiber as usize, Ordering::SeqCst);
    let done = f.fiber.resume(());
    CURRENT[cpu as usize].store(0, Ordering::SeqCst);
    f.saved_tls = crate::wasm::wt::platform::tls_raw_get();
    crate::wasm::wt::platform::tls_raw_set(prev);
    match done {
        Ok(code) => finish_fiber(f, code),  // return: il fiber è finito
        Err(())  => { /* suspended: l'hook futex l'ha già parcheggiato in WAITQ */ }
    }
    true
}

fn finish_fiber(f: Box<ThreadFiber>, code: i32) {
    let g = f.group.clone();
    if f.tid == 0 { *g.exit.lock() = Some(code); } // tid 0 = main
    if g.live.fetch_sub(1, Ordering::SeqCst) == 1 {
        // ultimo thread: il gruppo è finito; exec_threaded se ne accorge dal live==0
        crate::executor::wake_core(0);
    }
    crate::proc::unregister(f.tid_pid()); // vedi Task 6 (ps)
}
```
NOTE per l'esecutore: `tls_raw_get/set` sono da esporre in `platform.rs` (2 righe: load/store di `TLS[cpu_id()]`). `wasmtime_internal_fiber::Fiber::resume` ritorna `Result<Return, Yield>` (Ok=finito, Err=sospeso) — verifica la firma reale e adatta. `finish_fiber`/`tid_pid` completati nel Task 6; per ora stub senza proc.

- [ ] **Step 3: Seam in run_core**

`kernel/src/executor/mod.rs`, nel loop di `run_core` dopo il drain del pool (riga ~342):
```rust
// MT Fase 2: esegui i wasm-thread fiber runnable. Solo core ComputeApp
// (o il BSP sui sistemi 1-2 core, dove ComputeApp non esiste).
if crate::wasm::wt::threads::core_allowed(cpu) {
    while crate::wasm::wt::threads::run_one(cpu) {}
}
```
e nella disgiunzione wake-source (riga ~353):
```rust
let more = WAKE_PENDING[cpu as usize].load(Ordering::SeqCst)
    || crate::smp::inbox::is_pending(cpu)
    || !crate::smp::pool::is_empty()
    || (crate::wasm::wt::threads::core_allowed(cpu)
        && !crate::wasm::wt::threads::runnable_empty());
```
In `threads.rs`:
```rust
/// I fiber girano sui ComputeApp; fallback BSP quando non esistono (1-2 core).
pub fn core_allowed(cpu: u32) -> bool {
    match crate::cpu::core_role(cpu) {
        crate::cpu::CoreRole::ComputeApp => true,
        crate::cpu::CoreRole::BspIo => crate::cpu::first_compute_app_core().is_none(),
        _ => false,
    }
}
```

- [ ] **Step 4: Self-test host-only (niente wasm): fiber suspend/resume cross-core**

In `threads.rs`, gated boot-checks — crea un fiber che incrementa un contatore, si auto-sospende via una funzione `test_park()` (stesso meccanismo dell'hook futex ma su una chiave di test), viene risvegliato da `test_unpark()` chiamato dal BSP, e finisce. Assert: il fiber ha girato (contatore avanzato), il resume è avvenuto, e `ran_on` registra un core ≠ BSP con ≥3 core. Marker:
```rust
crate::binfo!("wt", "THREADS-FIBER-OK ran_on={} resumed={}", core, resumed);
```
Chiamato da `interrupts.rs` dopo il gate 1. (Codice: copia il protocollo park di Task 4 Step 1 ridotto a una chiave statica di test — l'esecutore lo scrive qui e lo riusa al Task 4.)

- [ ] **Step 5: Build + boot + marker**

Come Task 1 Step 5, grep `THREADS-FIBER-OK`. Expected: presente, `ran_on` ∈ core ComputeApp, niente panic. Anche `make run-test` resta verde.

- [ ] **Step 6: Commit** (`feat(wt): fiber runtime for wasm threads — stack-switch + TLS swap + run_core seam`).

---

### Task 3: exec_threaded — il main di un modulo threaded gira su fiber (route da run_cwasm)

**Files:**
- Modify: `kernel/src/wasm/wt/threads.rs` (`exec_threaded`), `kernel/src/wasm/wt/mod.rs` (route), `kernel/src/wasm/wt/state.rs` (env + threads handle), `kernel/src/wasm/wt/wasi.rs` (environ reali)

- [ ] **Step 1: WtState esteso**

`state.rs`:
```rust
pub struct WtState {
    pub args: Vec<Vec<u8>>,
    pub env: Vec<Vec<u8>>,                       // "K=V" bytes; vuoto per i tool classici
    pub exit: Option<i32>,
    pub fds: Vec<WtFd>,
    pub stdout_pty: Option<crate::vfs::Fd>,
    pub threads: Option<alloc::sync::Arc<crate::wasm::wt::threads::ThreadGroup>>,
}
```
`WtState::new` inizializza `env: Vec::new(), threads: None`. Sistemare i costruttori esistenti (compile-driven).

- [ ] **Step 2: environ reali in wasi.rs**

Sostituisci gli stub (wasi.rs:255-262) col pattern esatto di `args_sizes_get`/`args_get` (wasi.rs:233-254) ma su `caller.data().wasi().env` (campo `env`): `environ_sizes_get` scrive count e bytes totali (NUL inclusi), `environ_get` scrive i puntatori + le stringhe NUL-terminate. Copia la struttura del codice args adattando il campo.

- [ ] **Step 3: route in run_cwasm**

`wt/mod.rs`, in `run_cwasm` subito dopo il deserialize (riga ~226):
```rust
// MT Fase 2: un modulo wasm32-wasip1-threads importa la memoria condivisa
// (env::memory shared). Va eseguito sul runtime threaded (main su fiber),
// altrimenti un atomic.wait nel main non avrebbe un fiber da sospendere.
let wants_shared = module.imports().any(|i| {
    i.module() == "env" && i.name() == "memory"
        && i.ty().memory().map_or(false, |m| m.shared())
});
if wants_shared {
    return crate::wasm::wt::threads::exec_threaded(&module, args, pts);
}
```

- [ ] **Step 4: exec_threaded**

In `threads.rs`:
```rust
/// Esegue un modulo threaded: gruppo + SharedMemory + linker condiviso,
/// main (_start) su fiber tid=0, attesa cooperativa della fine del gruppo.
/// Chiamato dal task embassy run_app_on_core su un AP (o inline 1-2 core).
pub fn exec_threaded(module: &wasmtime::Module, args: Vec<Vec<u8>>, pts: Option<usize>) -> i32 {
    let engine = crate::wasm::wt::engine();
    let mem_ty = match module.imports().find_map(|i|
        if i.module()=="env" && i.name()=="memory" { i.ty().memory().cloned() } else { None }) {
        Some(t) => t, None => return 126,
    };
    let shared = match wasmtime::SharedMemory::new(engine, mem_ty) {
        Ok(s) => s, Err(e) => { crate::kprintln!("ruos: exec_threaded SharedMemory: {:?}", e); return 126; }
    };
    // Linker condiviso del gruppo: wasi + gfx + gui + thread-spawn + env::memory.
    let mut linker: wasmtime::Linker<crate::wasm::wt::state::WtState> = wasmtime::Linker::new(engine);
    if crate::wasm::wt::wasi::add_to_linker(&mut linker).is_err() { return 126; }
    let _ = crate::wasm::wt::gfx::add_to_linker(&mut linker);
    let _ = crate::wasm::wt::gui::add_to_linker(&mut linker);
    if add_thread_spawn_to_linker(&mut linker).is_err() { return 126; }   // Task 4
    if linker.define_unknown_imports_as_traps(module).is_err() {}          // opzionale, come upstream
    // NB define della memoria: serve una Store per il define? In wasmtime 45
    // linker.define richiede &Store solo per item non-host; SharedMemory è
    // engine-scoped → usare la variante che non richiede store se esiste,
    // altrimenti definirla per-store prima di ogni instantiate (helper sotto).
    let ncomp = crate::cpu::cpus_online().saturating_sub(2).max(1);
    let mut env: Vec<Vec<u8>> = Vec::new();
    env.push(alloc::format!("RAYON_NUM_THREADS={}", ncomp).into_bytes());
    let group = Arc::new(ThreadGroup {
        module: module.clone(), linker: Arc::new(linker), shared,
        next_tid: AtomicU32::new(1), live: AtomicU32::new(0),
        poisoned: AtomicBool::new(false), exit: IrqMutex::new(None),
        base_args: args, env,
    });
    // main = tid 0
    spawn_fiber(group.clone(), 0, 0 /*start_arg unused per il main*/, pts);
    // Attesa cooperativa: questo è un task embassy sull'AP — yield finché il
    // gruppo non muore (live==0). Periodo 1 tick.
    loop {
        if group.live.load(Ordering::SeqCst) == 0 {
            return group.exit.lock().unwrap_or(if group.poisoned.load(Ordering::SeqCst) {134} else {0});
        }
        // NB: exec_threaded è sync dentro run_app_on_core (come run_cwasm);
        // qui non c'è await: hlt-yield breve per non bruciare il core.
        core::hint::spin_loop();
        x86_64::instructions::hlt();
    }
}

/// Crea il fiber di UN thread (main tid=0 o spawned tid>0) e lo accoda runnable.
fn spawn_fiber(group: Arc<ThreadGroup>, tid: u32, start_arg: i32, pts: Option<usize>) {
    group.live.fetch_add(1, Ordering::SeqCst);
    let stack = wasmtime_internal_fiber::FiberStack::new(FIBER_STACK_SIZE).expect("fiber stack");
    let g = group.clone();
    let fiber = wasmtime_internal_fiber::Fiber::new(stack, move |_resume: (), suspend| -> i32 {
        // Pubblica il Suspend per l'hook futex (vedi CURRENT in run_one).
        publish_suspend(suspend);
        // Store + WtState del thread.
        let engine = crate::wasm::wt::engine();
        let mut state = crate::wasm::wt::state::WtState::new(g.base_args.clone());
        state.env = g.env.clone();
        state.threads = Some(g.clone());
        if tid == 0 { if let Some(n) = pts { /* stdout_pty: stesso codice di run_cwasm:228-234 */ } }
        let mut store = wasmtime::Store::new(engine, state);
        store.set_epoch_deadline(crate::wasm::wt::NO_DEADLINE_TICKS);
        // env::memory del gruppo per QUESTA store, poi instantiate.
        // (helper: define_shared(&g.linker, &mut store, &g.shared) — vedi nota Step 4)
        let inst = match instantiate_in_group(&g, &mut store) {
            Ok(i) => i, Err(e) => { crate::kprintln!("ruos: thread tid={} instantiate: {:?}", tid, e);
                                    g.poisoned.store(true, Ordering::SeqCst); return 126; }
        };
        unsafe { core::arch::asm!("cld", options(nostack)); } // DF=0 come run_cwasm:261
        let r = if tid == 0 {
            inst.get_typed_func::<(), ()>(&mut store, "_start")
                .and_then(|f| f.call(&mut store, ()).map_err(Into::into))
        } else {
            inst.get_typed_func::<(i32, i32), ()>(&mut store, "wasi_thread_start")
                .and_then(|f| f.call(&mut store, (tid as i32, start_arg)).map_err(Into::into))
        };
        match r {
            Ok(()) => store.data().exit.unwrap_or(0),
            Err(e) => {
                if store.data().exit.is_some() { store.data().exit.unwrap() } // proc_exit
                else {
                    crate::bwarn!("wt", "thread tid={} trap: {:?} — kill group", tid, e);
                    g.poisoned.store(true, Ordering::SeqCst);
                    134
                }
            }
        }
    }).expect("fiber new");
    enqueue_runnable(Box::new(ThreadFiber {
        fiber, saved_tls: core::ptr::null_mut(),
        suspend_ptr: AtomicUsize::new(0), group, tid,
    }));
}
```
NOTE esecutore: (1) `instantiate_in_group` = helper che definisce `env::memory = g.shared.clone()` nel linker/per-store e poi `linker.instantiate(&mut store, &g.module)` — la firma esatta di `Linker::define` per SharedMemory in wasmtime 45 va verificata sul fork (se richiede `&Store`, definiscila qui per-store; un define ripetuto sullo stesso linker fallisce → in quel caso usa `linker.define` una sola volta in exec_threaded con una store usa-e-getta, o `Linker::allow_shadowing(true)`). (2) `publish_suspend` salva `suspend as *mut _ as usize` nel `ThreadFiber.suspend_ptr` corrente — il fiber NON ha accesso diretto alla sua Box; usa `CURRENT[cpu_id()]` che `run_one` ha appena settato. (3) il busy-wait di exec_threaded sull'AP: accettabile v1 (l'AP è dedicato all'app, identico a run_cwasm sync); annotare.

- [ ] **Step 5: Build verde + run-test verde** (niente ancora chiama thread-spawn: i moduli wasip1 classici non passano dalla route).

- [ ] **Step 6: Commit** (`feat(wt): exec_threaded — threaded modules run their main on a fiber`).

---

### Task 4: Hook futex (atomic.wait/notify) + Gate 3 (THREADS-OK 3)

**Files:**
- Modify: `kernel/src/wasm/wt/threads.rs` (hook + park/unpark), `kernel/src/boot/phases/interrupts.rs`
- Create: `tools/wt-threads-gate/gate3.wat` + regola Makefile + include/runner (pattern Task 1)

- [ ] **Step 1: Protocollo futex in threads.rs**

```rust
/// Spin adattivo prima del park: la critical section media è più corta del park.
const SPIN_ITERS: u32 = 200;

#[unsafe(no_mangle)]
pub extern "C" fn wasmtime_futex_wait32(addr: *const u32, expected: u32, timeout_ns: i64) -> u32 {
    // Fast path: spin con PAUSE ricontrollando il valore.
    for _ in 0..SPIN_ITERS {
        if unsafe { core::ptr::read_volatile(addr) } != expected { return 1; } // not-equal
        core::hint::spin_loop();
    }
    park_current(addr as usize, timeout_ns, || unsafe { core::ptr::read_volatile(addr) } != expected)
}

#[unsafe(no_mangle)]
pub extern "C" fn wasmtime_futex_wait64(addr: *const u64, expected: u64, timeout_ns: i64) -> u32 {
    for _ in 0..SPIN_ITERS {
        if unsafe { core::ptr::read_volatile(addr) } != expected { return 1; }
        core::hint::spin_loop();
    }
    park_current(addr as usize, timeout_ns, || unsafe { core::ptr::read_volatile(addr) } != expected)
}

#[unsafe(no_mangle)]
pub extern "C" fn wasmtime_futex_notify(addr: *const u8, count: u32) -> u32 {
    let key = addr as usize;
    let mut woken = 0u32;
    let mut to_run: Vec<Box<ThreadFiber>> = Vec::new();
    {
        let mut s = shard(key).0.lock();
        let mut i = 0;
        while i < s.len() && woken < count {
            if s[i].0 == key { let (_, _, f) = s.swap_remove(i); to_run.push(f); woken += 1; }
            else { i += 1; }
        }
    } // lock dropped prima dell'enqueue (che fa IPI)
    for f in to_run { RUNQ.lock().push_back(f); }
    if woken > 0 { crate::apic::lapic::send_ipi_all_but_self(crate::idt::VEC_WAKE); }
    woken
}

/// Parcheggia il fiber corrente sulla chiave `key`. Protocollo anti-lost-wakeup:
/// (1) lock shard → (2) ricontrolla la condizione SOTTO il lock → (3) se ancora
/// da attendere, sposta il fiber in WAITQ e SOSPENDI (il lock è droppato PRIMA
/// del suspend: il fiber è già visibile a notify, che lo rimetterà in RUNQ).
/// return: 0=woken, 1=not-equal (ricontrollo), 2=timeout.
fn park_current(key: usize, timeout_ns: i64, not_equal: impl Fn() -> bool) -> u32 {
    let cpu = crate::cpu::cpu_id() as usize;
    let me = CURRENT[cpu].load(Ordering::SeqCst) as *mut ThreadFiber;
    if me.is_null() {
        // Nessun fiber (es. chiamata da contesto non-thread): degrada a spin.
        // Non deve succedere (i moduli threaded girano SEMPRE su fiber), ma
        // non deve neanche deadlockare il kernel.
        while not_equal() == false { core::hint::spin_loop(); }
        return 1;
    }
    let deadline = if timeout_ns < 0 { u64::MAX }
        else { crate::timer::ticks() + (timeout_ns as u64 / 10_000_000).max(1) }; // ns→tick 100Hz
    {
        let mut s = shard(key).0.lock();
        if not_equal() { return 1; } // ricontrollo sotto lock
        // Il MOVE della Box avviene in run_one al ritorno del suspend: qui
        // registriamo solo (key, deadline); run_one vede Err(suspended) e
        // inserisce la Box in WAITQ usando questi parametri.
        unsafe { (*me).park_key = key; (*me).park_deadline = deadline; }
    }
    // Sospendi: torna a run_one (Err). Al resume, notify ci ha rimesso in RUNQ.
    let sus = unsafe { (*me).suspend_ptr.load(Ordering::SeqCst) as *mut FiberSuspend };
    let woke = unsafe { (*sus).suspend(()) }; // tipo reale: Suspend<(), (), i32>
    let _ = woke;
    if crate::timer::ticks() >= deadline && deadline != u64::MAX { 2 } else { 0 }
}
```
NOTE esecutore (importanti):
1. Aggiungi a `ThreadFiber` i campi `park_key: usize, park_deadline: u64` (0 = non parcheggiato). `run_one`, quando `resume` ritorna `Err(suspended)`, fa: se `park_key != 0` → push `(park_key, park_deadline, f)` nello shard e azzera `park_key`; altrimenti (suspend spurio) re-enqueue in RUNQ. COSÌ la Box si muove fuori-fiber, mai dentro.
2. Race window: tra il drop del lock shard e l'effettivo park in run_one, un notify può NON trovare il fiber nello shard (è ancora "in volo"). Fix: il registro in WAITQ avviene PRIMA del suspend → sposta l'inserimento nello shard DENTRO `park_current` (sotto il lock) usando un placeholder; ma la Box è posseduta da run_one… **Soluzione canonica**: `park_current` setta `park_key/deadline` e sospende; `run_one` inserisce in WAITQ; `wasmtime_futex_notify` che non trova la chiave nello shard setta un flag `pending_wake` in una mappa shard `(key→contatore)`: `run_one`, PRIMA di inserire in WAITQ, controlla/consuma `pending_wake` per la sua chiave e in caso re-enqueue subito. Implementa questo contatore nello stesso shard (campo `Vec<(usize, u32)>` o piccola mappa). Documenta l'invariante nel codice.
3. `FiberSuspend` = alias del tipo Suspend reale del crate. Timeout: i waiter scaduti vanno riscattati — aggiungi `pub fn expire_timeouts()` che scorre gli shard con `ticks() >= deadline` e re-enqueue (chiamata da run_core ogni giro, costo ~0 con `EARLIEST_DEADLINE: AtomicU64` come pre-filtro). Scrivila.

- [ ] **Step 2: Gate 3 guest**

`tools/wt-threads-gate/gate3.wat` — DUE "thread" senza spawn ancora: il main fa `atomic.notify` dopo un ciclo; il gate runner host crea il gruppo e spawna A MANO due fiber sullo stesso modulo (riusa `spawn_fiber` con due export diversi `waiter`/`waker`):
```wat
(module
  (import "env" "memory" (memory 1 1 shared))
  (func (export "waiter") (result i32)
    ;; wait su mem[32] finché vale 0; al wake ritorna mem[36]
    (drop (memory.atomic.wait32 (i32.const 32) (i32.const 0) (i64.const -1)))
    (i32.atomic.load (i32.const 36)))
  (func (export "waker") (result i32)
    (i32.atomic.store (i32.const 36) (i32.const 7))
    (i32.atomic.store (i32.const 32) (i32.const 1))
    (drop (memory.atomic.notify (i32.const 32) (i32.const 1)))
    (i32.const 0)))
```
Runner kernel (boot-checks): crea gruppo per gate3.cwasm, spawna fiber-waiter poi fiber-waker (export custom — generalizza `spawn_fiber` con un parametro export/firma per il path di test), attendi `live==0`, assert: waiter exit = 7. Marker `THREADS-OK 3 = ok`. **Questo prova: wait sospende il fiber (il waiter NON blocca il core: il waker gira), notify risveglia via IPI.**

- [ ] **Step 3: Makefile + include + boot call** (pattern identico Task 1 Step 2-4, file `threads_gate3.cwasm`).

- [ ] **Step 4: Boot + assert** `THREADS-OK 3 = ok` con `-smp 4 -m 2048`; anche con `-smp 2` (fallback BSP) non deve deadlockare.

- [ ] **Step 5: Commit** (`feat(wt): futex hooks — atomic.wait suspends the fiber, notify wakes via IPI`).

---

### Task 5: thread-spawn + Gate 2 (THREADS-OK 2)

**Files:**
- Modify: `kernel/src/wasm/wt/threads.rs` (`add_thread_spawn_to_linker`), `kernel/src/wasm/wt/wasi.rs` (registrazione condizionale)
- Create: `tools/wt-threads-gate/gate2.wat` + regola/include/runner

- [ ] **Step 1: Host fn thread-spawn**

In `threads.rs`:
```rust
/// Registra l'import wasi-threads: modulo "wasi", field "thread-spawn" (TRATTINO).
pub fn add_thread_spawn_to_linker(
    linker: &mut wasmtime::Linker<crate::wasm::wt::state::WtState>,
) -> Result<(), wasmtime::Error> {
    linker.func_wrap("wasi", "thread-spawn",
        |caller: wasmtime::Caller<'_, crate::wasm::wt::state::WtState>, start_arg: i32| -> i32 {
            let g = match caller.data().threads.clone() {
                Some(g) => g, None => return -1, // ENOTSUP: modulo non threaded
            };
            if g.poisoned.load(Ordering::SeqCst) { return -1; }
            let tid = g.next_tid.fetch_add(1, Ordering::SeqCst);
            if tid >= (1 << 29) { return -1; } // range wasi-threads
            crate::binfo!("wt", "thread-spawn tid={} live={}", tid,
                          g.live.load(Ordering::SeqCst) + 1);
            spawn_fiber(g, tid, start_arg, None);
            tid as i32
        })?;
    Ok(())
}
```
Spawn NON esegue inline: accoda il fiber (run-to-runnable, lo prende il primo core libero). Già wired in `exec_threaded` (Task 3).

- [ ] **Step 2: Gate 2 guest**

`gate2.wat` — il main spawna un "thread" via l'import e attende che scriva una cella:
```wat
(module
  (import "env" "memory" (memory 1 1 shared))
  (import "wasi" "thread-spawn" (func $spawn (param i32) (result i32)))
  (func (export "wasi_thread_start") (param $tid i32) (param $arg i32)
    (i32.atomic.store (i32.const 64) (i32.const 99))
    (drop (memory.atomic.notify (i32.const 64) (i32.const 1))))
  (func (export "run") (result i32)
    (drop (call $spawn (i32.const 0)))
    ;; aspetta che il thread scriva 99
    (drop (memory.atomic.wait32 (i32.const 64) (i32.const 0) (i64.const -1)))
    (i32.atomic.load (i32.const 64))))
```
Runner: gruppo + fiber main su export `run` (riusa il path di test del Task 4), assert exit = 99 → `THREADS-OK 2 = ok`. Prova: spawn crea una nuova Instance sulla STESSA SharedMemory, il child scrive, il main lo vede.

- [ ] **Step 3: Makefile + include + boot call + assert** (pattern solito). Tutti e tre i marker ora nel log: `THREADS-OK 1/2/3`.

- [ ] **Step 4: Aggiorna `tests/` — script gate**

Create `tests/threads-test.sh` (clone della struttura di frame-smp-test.sh): builda con `CARGO_FEATURES=boot-checks`, boota `-smp 4 -m 2048`, grep-assert i 3 marker `THREADS-OK [123] = ok` + `THREADS-FIBER-OK`. FAIL se uno manca.

- [ ] **Step 5: Commit** (`feat(wt): wasi thread-spawn — new instance per thread on the shared memory`).

---

### Task 6: parsum end-to-end (std::thread + rayon) + ps tids

**Files:**
- Create: `tools/parsum/` (crate rust wasm32-wasip1-threads)
- Modify: `Makefile` (build/parsum.cwasm + staging /bin), `kernel/src/wasm/wt/threads.rs` (proc), `kernel/src/proc.rs` (niente: già IrqMutex)

- [ ] **Step 1: parsum**

`tools/parsum/Cargo.toml`:
```toml
[package] name = "parsum"; version = "0.1.0"; edition = "2021"
[dependencies] rayon = "1"
[profile.release] opt-level = 3
```
`tools/parsum/src/main.rs`:
```rust
use rayon::prelude::*;
fn main() {
    let n: u64 = 50_000_000;
    let t0 = std::time::Instant::now();
    let serial: u64 = (0..n).map(|x| x ^ (x >> 3)).sum();
    let t_ser = t0.elapsed();
    let t1 = std::time::Instant::now();
    let parallel: u64 = (0..n).into_par_iter().map(|x| x ^ (x >> 3)).sum();
    let t_par = t1.elapsed();
    assert_eq!(serial, parallel);
    let speedup_x100 = (t_ser.as_micros() * 100 / t_par.as_micros().max(1)) as u64;
    println!("PARSUM_OK threads={} sum={} speedup_x100={}",
             rayon::current_num_threads(), parallel, speedup_x100);
}
```
(`Instant` su wasi usa `clock_time_get` monotonic — già implementato in wasi.rs:269.)

- [ ] **Step 2: Build rule Makefile**

```make
build/parsum.cwasm: tools/parsum/src/main.rs tools/parsum/Cargo.toml $(WT_PRECOMPILE)
	@mkdir -p build
	source $$HOME/.cargo/env && cd tools/parsum && \
		cargo build --release --target wasm32-wasip1-threads
	$(WT_PRECOMPILE) tools/parsum/target/wasm32-wasip1-threads/release/parsum.wasm build/parsum.cwasm
```
Aggiungi `build/parsum.cwasm` alle dipendenze del target `iso` + copia in `build/binstage/` (pattern wtecho, Makefile:120-122 + 280-293).

- [ ] **Step 3: Init script + run**

`user-bin/threads-init.sh`:
```sh
parsum
echo ruos boot OK
```
Run:
```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && make iso INIT_SCRIPT=user-bin/threads-init.sh && timeout 120 qemu-system-x86_64 -machine q35 -cpu max -smp 6 -m 2048 -cdrom build/os.iso -serial file:build/parsum.log -display none -no-reboot -device qemu-xhci >/dev/null 2>&1; grep -a "PARSUM_OK" build/parsum.log'
```
Expected: `PARSUM_OK threads=4 sum=… speedup_x100=…` (6 vCPU → 4 ComputeApp). `threads=4` prova che RAYON_NUM_THREADS è arrivato. speedup > 100 atteso ma NON asserito hard in QEMU TCG (timing inaffidabile); assert solo su `threads>=2` e sum corretto. **Se hang/deadlock**: netconsole + marker, debug sistematico — è il primo carico std reale.

- [ ] **Step 4: ps tids**

In `spawn_fiber`: `let pid = crate::proc::register(alloc::format!("{}#{}", base_name, tid));` con `base_name` nel ThreadGroup (derivato dall'argv[0] in exec_threaded); salva `pid` nel ThreadFiber; `finish_fiber` fa `crate::proc::unregister(pid)`. Verifica manuale: `ps` dalla shell durante un parsum lungo mostra `parsum#1..N`.

- [ ] **Step 5: tests/threads-test.sh — aggiungi lo stage parsum** (seconda metà dello script: boot con threads-init, assert `PARSUM_OK threads=[2-9]`).

- [ ] **Step 6: Commit** (`feat(wt): parsum — std::thread+rayon end-to-end on wasm32-wasip1-threads`).

---

### Task 7: Stress mutex conteso + kill-group + regressione + docs

**Files:**
- Create: `tools/mtstress/`
- Modify: `kernel/src/wasm/wt/threads.rs` (kill-group), `docs/api/` (pagina wasi: thread-spawn + environ), `docs/superpowers/specs/2026-06-12-wasm-mt-fase2-threads-design.md` (esiti), `CHANGELOG/NNN-…`

- [ ] **Step 1: mtstress**

`tools/mtstress/src/main.rs` (std, wasm32-wasip1-threads):
```rust
use std::sync::{Arc, Mutex};
fn main() {
    let n_threads = 4; let m = 100_000u64;
    let counter = Arc::new(Mutex::new(0u64));
    let hs: Vec<_> = (0..n_threads).map(|_| {
        let c = counter.clone();
        std::thread::spawn(move || { for _ in 0..m { *c.lock().unwrap() += 1; } })
    }).collect();
    for h in hs { h.join().unwrap(); }
    let v = *counter.lock().unwrap();
    assert_eq!(v, n_threads as u64 * m);
    println!("STRESS_MT_OK count={}", v);
}
```
Regola Makefile identica a parsum. Esegue: Mutex conteso (futex wait/notify reali), join (futex), valore ESATTO = prova atomicità + coerenza. Boot + assert `STRESS_MT_OK count=400000`.

- [ ] **Step 2: Kill-group su trap**

In `run_one`/`finish_fiber`: se `group.poisoned` → (a) i fiber del gruppo in RUNQ vengono droppati al take (check in `run_one` subito dopo il pop: `if f.group.poisoned.load() { finish_fiber(f, 134); return true; }`); (b) i waiter in WAITQ del gruppo vanno rimossi: aggiungi `pub fn kill_group_waiters(g: &Arc<ThreadGroup>)` che scorre gli shard e droppa i fiber con `Arc::ptr_eq(&f.group, g)` (chiamata da chi setta poisoned). Droppare un fiber sospeso = Drop della Box (lo stack TryVec si libera; le Store dentro muoiono — accettato: il gruppo è morto). Test: variante mtstress con un thread che fa `unreachable` → il gruppo esce 134, il kernel NON panica, `ps` pulito. Aggiungi al test sh.

- [ ] **Step 3: Regressione completa**

```bash
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && make run-test && bash tests/frame-smp-test.sh && bash tests/threads-test.sh && bash tests/comp-smp-test.sh'
```
Expected: run-test PASS, frame-smp PASS, threads PASS; comp-smp: band `>=2` PASS (lo screendump-equivalence ha un fail pre-esistente noto — changelog 476 — non bloccare su quello). Overlay `wm-fps`: boot GUI con un mtstress in loop → desktop fluido, fps stabile (degrado solo dei core compute).

- [ ] **Step 4: VBox (OBBLIGATORIO, CPU-sensitive)**

ISO con boot-checks + threads-init; ≥4 vCPU + 2048 MB. Verifica manuale utente: marker THREADS-OK 1/2/3, PARSUM_OK, STRESS_MT_OK, desktop ok. (Chiedere all'utente, come Fase 1.)

- [ ] **Step 5: Docs**

- `docs/api/` pagina del modulo `wasi` (o crearla): import `("wasi","thread-spawn")` (signature, semantica, errori), environ ora reale, nota `RAYON_NUM_THREADS` iniettato. "Last reviewed" aggiornato. (Regola CLAUDE.md: stesso commit della host fn — se il Task 5 è già committato, questo è il commit di sanatoria esplicita: meglio fare la pagina NEL Task 5; spostala lì se esegui in ordine.)
- Spec Fase 2: stato → implementato, esiti gate, deviazioni documentate (NO_DEADLINE per thread store; niente async_support — fiber nostri; fork vendorizzato).
- `build-iso.ps1`: aggiungi `rustup target add wasm32-wasip1-threads` allo step 2 (riga ~215, accanto agli altri target).
- `CLAUDE.md`: aggiorna la riga toolchain WSL (target `wasm32-wasip1-threads`) e la sezione "Due runtime WASM" (thread nelle app .cwasm).
- Changelog finale fase.

- [ ] **Step 6: Commit finale** (`feat(wt): MT Fase 2 — std::thread/rayon in .cwasm apps via M:N fibers`).

---

## Rischi & punti di stop espliciti

1. **Task 0 Step 4** (fork compila in no_std) e **Task 1 Step 5** (gate 1) sono i cancelli hard: se non passano, STOP e debug lì — niente fallback (decisione utente).
2. **Race notify-vs-park** (Task 4 Step 1 nota 2): implementare ESATTAMENTE il protocollo pending_wake descritto; è il bug di concorrenza più probabile della fase.
3. **TLS swap** (Task 2): se un trap/epoch unwinda mentre il TLS è swappato, lo stato per-core deve restare coerente — il ripristino in `run_one` è nel path comune (sia Ok sia Err), verificato dal gate watchdog del Task 7 Step 2.
4. **`Linker::define` di SharedMemory**: API esatta da verificare sul fork (nota Task 3 Step 4). Se serve per-store, l'helper `instantiate_in_group` la definisce a ogni instantiate.
5. **Compat .cwasm esistenti** dopo `wasm_threads(true)`: verificata al Task 0 Step 7; se l'hash cambia → re-AOT sistemico + nota changelog per `/mnt/apps`.

## Self-Review (eseguito)

- **Spec coverage:** §1 gate 3 punti → Task 1 (gate1), Task 5 (gate2), Task 4 (gate3) + fiber self-test Task 2 (il punto (3) è soddisfatto dai fiber nostri — deviazione da "async_support" documentata, stesso requisito comportamentale). §2 toolchain/engine → Task 0. §3 SharedMemory no_std → Task 0 fork. §4 demand paging → nessuna modifica (premesse). §5 scheduler → Task 2+3+4. §6 spawn policy/oversubscription → Task 5 (spawn oltre i core = consentito: la RUNQ accoda). §7 watchdog/kill → Task 7 Step 2 (kill-group) + deviazione NO_DEADLINE documentata. §8 ps → Task 6 Step 4. §9 performance → spin PAUSE (Task 4), align(64) (Task 2), atomics nativi (fork+AOT); likely/#[cold] applicabili in rifinitura. §10 fuori scope rispettato. §11 test → threads-test.sh + parsum + mtstress + regressione + VBox. §12 rischi → sezione sopra.
- **Placeholder scan:** i punti "verifica API reale e adatta" sono investigation-step espliciti con outcome atteso, non TODO ciechi — inevitabili su un fork non ancora scritto; ogni altro step ha codice concreto.
- **Type consistency:** `ThreadGroup`/`ThreadFiber`/`spawn_fiber`/`exec_threaded`/`run_one`/`core_allowed`/`wasmtime_futex_*` coerenti tra i task; `WtState.env`+`threads` introdotti al Task 3 e usati nei Task 5-6.
