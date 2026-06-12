# MT Fase 2 — Task 2: Fiber Runtime (threads.rs nucleo) + self-test

> **Per la prossima sessione:** questo file riassume lo stato e il piano di
> implementazione del Task 2. I Task 0 (changelog 486) e Task 1 (changelog 487)
> sono già completati. Il piano completo è in
> `docs/superpowers/plans/2026-06-12-wasm-mt-fase2-threads.md`.

## Stato attuale

| Task | Stato | Changelog |
|------|-------|-----------|
| 0 — Vendor fork + build verde | ✅ Done | 486 |
| 1 — Gate 1 (SharedMemory + atomics) | ✅ Done | 487 |
| 2 — Fiber runtime + self-test | ✅ Done | 488 |
| 3 — exec_threaded (main on fiber) | ✅ Done | 489 |
| 4 — Futex hooks (atomic.wait/notify) + Gate 3 | ✅ Done | 490 |
| 5 — thread-spawn + Gate 2 | ✅ Done | 491 |
| **6 — parsum end-to-end (rayon)** | ⬜ **NEXT** | — |
| 7 — Stress + kill-group + docs | ⬜ Pending | — |

## File da toccare nel Task 2

### 1. [MODIFY] `kernel/Cargo.toml`

Aggiungere dep:
```toml
# Stack-switch fiber per i wasm-thread (MT Fase 2). Backend no_std upstream.
wasmtime-internal-fiber = { version = "=45.0.0", default-features = false }
```

### 2. [NEW] `kernel/src/wasm/wt/threads.rs`

Nucleo dello scheduler fiber M:N. Strutture principali:

- **`ThreadFiber`**: fiber + saved TLS + suspend handle + group ref + tid + park state (park_key, park_deadline)
- **`ThreadGroup`**: stato condiviso di UNA app threaded (Module + SharedMemory + Linker + env + atomics: next_tid, live, poisoned, exit)
- **`RUNQ`**: run-queue globale dei fiber runnable (`IrqMutex<VecDeque<Box<ThreadFiber>>>`)
- **`WAITQ`**: wait-queue futex sharded per indirizzo (`[WaitShard; 16]`, `align(64)` anti false-sharing)
- **`CURRENT`**: per-core pointer al fiber correntemente in esecuzione (`[AtomicUsize; MAX_CPUS]`)

Funzioni:
- `run_one(cpu) -> bool`: dequeue + TLS swap + resume + handle Ok(finito)/Err(sospeso)
- `enqueue_runnable(f)`: push_back + IPI broadcast `send_ipi_all_but_self(VEC_WAKE)`
- `core_allowed(cpu) -> bool`: ComputeApp oppure fallback BSP (quando non esistono ComputeApp)
- `runnable_empty() -> bool`: predicato per il wake-source check
- `finish_fiber(f, code)`: decrementa `live`, se ultimo thread sveglia il BSP
- Boot-check: `fiber_self_test()` — crea un fiber host-only che incrementa un counter, si auto-sospende, viene ripreso, e finisce. Assert: counter avanzato, resume avvenuto.

#### API wasmtime-internal-fiber verificata (v45.0.0, backend no_std)

```rust
// FiberStack
FiberStack::new(size: usize, zeroed: bool) -> Result<FiberStack>

// Fiber<'a, Resume, Yield, Return>
Fiber::new(stack, func: FnOnce(Resume, &mut Suspend<Resume, Yield, Return>) -> Return) -> Result<Fiber>
fiber.resume(val: Resume) -> Result<Return, Yield>  // Ok = finito, Err = sospeso

// Suspend<Resume, Yield, Return>
suspend.suspend(value: Yield) -> Resume
```

- `Fiber` è `!Send` (contiene `Cell`). `ThreadFiber` la wrappa in Box con `unsafe impl Send` — safe perché solo un core alla volta tocca un fiber (ownership esclusiva in `run_one`).
- `FiberStack::new` prende `(size, zeroed)` — due args. Usare `zeroed: false`.
- Il crate dipende da `wasmtime-environ` e `cfg-if`. Il backend no_std usa `TryVec<u8>` per lo stack (no guard pages, no mmap). Asm in `stackswitch.rs`.

#### Protocollo TLS swap in `run_one`

```rust
pub fn run_one(cpu: u32) -> bool {
    let mut f = match RUNQ.lock().pop_front() { Some(f) => f, None => return false };
    // TLS swap: salva il TLS del core, imposta quello del fiber
    let prev = crate::wasm::wt::platform::tls_raw_get();
    crate::wasm::wt::platform::tls_raw_set(f.saved_tls);
    CURRENT[cpu as usize].store(&mut *f as *mut ThreadFiber as usize, Ordering::SeqCst);
    let done = f.fiber.resume(());
    CURRENT[cpu as usize].store(0, Ordering::SeqCst);
    f.saved_tls = crate::wasm::wt::platform::tls_raw_get();
    crate::wasm::wt::platform::tls_raw_set(prev);
    match done {
        Ok(code) => finish_fiber(f, code),  // fiber finito
        Err(())  => { /* sospeso: l'hook futex l'ha parcheggiato in WAITQ */ }
    }
    true
}
```

### 3. [MODIFY] `kernel/src/wasm/wt/platform.rs`

Esporre helper interni per il TLS swap (senza `extern "C"` / `#[no_mangle]`):

```rust
/// Raw TLS load for the fiber scheduler's TLS swap (threads.rs).
pub fn tls_raw_get() -> *mut u8 {
    TLS[crate::cpu::cpu_id() as usize].load(Ordering::SeqCst)
}

/// Raw TLS store for the fiber scheduler's TLS swap (threads.rs).
pub fn tls_raw_set(ptr: *mut u8) {
    TLS[crate::cpu::cpu_id() as usize].store(ptr, Ordering::SeqCst);
}
```

### 4. [MODIFY] `kernel/src/wasm/wt/mod.rs`

Aggiungere `pub mod threads;` dopo le dichiarazioni di modulo esistenti (dopo `pub mod net;`, riga 17).

### 5. [MODIFY] `kernel/src/executor/mod.rs`

In `run_core()` (riga ~325, nel loop), dopo il pool drain (riga ~342-344):
```rust
// MT Fase 2: esegui i wasm-thread fiber runnable. Solo core ComputeApp
// (o il BSP sui sistemi 1-2 core, dove ComputeApp non esiste).
if crate::wasm::wt::threads::core_allowed(cpu) {
    while crate::wasm::wt::threads::run_one(cpu) {}
}
```

Nella disgiunzione wake-source (riga ~353-355), aggiungere:
```rust
let more = WAKE_PENDING[cpu as usize].load(Ordering::SeqCst)
    || crate::smp::inbox::is_pending(cpu)
    || !crate::smp::pool::is_empty()
    || (crate::wasm::wt::threads::core_allowed(cpu)
        && !crate::wasm::wt::threads::runnable_empty());
```

### 6. [MODIFY] `kernel/src/boot/phases/interrupts.rs`

Nel blocco boot-checks, dopo il marker `THREADS-OK 1` (riga ~212):
```rust
// MT Fase 2 fiber self-test: host-only fiber suspend/resume cross-core.
let fiber_ok = crate::wasm::wt::threads::fiber_self_test();
crate::binfo!("wt", "THREADS-FIBER-OK = {}", if fiber_ok { "ok" } else { "FAIL" });
```

---

## Note tecniche importanti

### Riferimenti nel codebase

| Cosa | Dove |
|------|------|
| `IrqMutex` | `kernel/src/sync/mod.rs:14` — spinlock con IRQ disable |
| `cpu_id()` | `kernel/src/cpu/mod.rs:281` — via RDTSCP |
| `MAX_CPUS` | `kernel/src/cpu/mod.rs` — costante per gli array per-core |
| `core_role(cpu)` | `kernel/src/cpu/mod.rs:64` — `BspIo`, `GuiCompositor`, `ComputeApp` |
| `first_compute_app_core()` | `kernel/src/cpu/mod.rs:80` |
| `ticks()` | `kernel/src/timer.rs:55` — 100 Hz, 1 tick = 10 ms |
| `VEC_WAKE` | `kernel/src/idt.rs` — vettore IPI per sveglia core |
| `send_ipi_all_but_self` | `kernel/src/apic/lapic.rs:77` |
| `TLS` per-core array | `kernel/src/wasm/wt/platform.rs:28-31` |
| `run_core` loop | `kernel/src/executor/mod.rs:272-369` |
| Pool drain seam | `kernel/src/executor/mod.rs:342-344` |
| Wake-source check | `kernel/src/executor/mod.rs:353-355` |

### Verifica

```bash
# Build check
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos/kernel && source ~/.cargo/env && cargo check --target x86_64-unknown-none -Zbuild-std=core,alloc 2>&1 | tail -20'

# Boot test con marker
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && make iso CARGO_FEATURES=boot-checks && timeout 60 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -m 2048 -cdrom build/os.iso -serial file:build/fiber.log -display none -no-reboot -device qemu-xhci >/dev/null 2>&1; grep -a "THREADS-FIBER-OK\|THREADS-OK" build/fiber.log'

# Regressione
wsl -d Ubuntu-22.04 -u root -e bash -c 'cd /mnt/w/Work/GitHub/ruos && make run-test'
```

### Commit message
```
feat(wt): fiber runtime for wasm threads — stack-switch + TLS swap + run_core seam
```

Changelog: `CHANGELOG/488-26-06-12-wt-threads-fiber-runtime.md`
