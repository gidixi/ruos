# Step 9 — Async executor cooperativo (embassy + custom Pender)

**Data:** 2026-05-28
**Roadmap step:** 9
**Stato:** spec approvata, da implementare

## Obiettivo

Sostituire `vfs::block_on` (noop_waker, single-future poll-busy) con un
executor cooperativo vero. Il timer IRQ a 100 Hz già attivo diventa la
sorgente di wake per future temporali; la keyboard IRQ diventa la sorgente
di wake per future di input.

Questo step elimina lo "scheduler stub" lasciato a Step 7 e abilita
multi-task cooperativo necessario per Step 10 (WASM runtime) e Step 11
(shell), entrambi dei quali dovranno essere structured come task async.

## Non-obiettivi (rimandati)

- **Preemption** — esplicitamente droppato dal pivot WASM. Concurrency =
  cooperative only.
- **SMP / multi-CPU** — single-CPU, single-threaded executor.
- **`embassy-time` (Duration/Instant/Timer)** — YAGNI fino a Step 14
  (TCP timeouts). Step 9 usa una `Delay(target_ticks)` minimale hand-roll.
- **Dynamic spawn / `Box<dyn Future>`** — task slab statica di embassy
  basta per gli use-case di Step 9-11.

## Decisioni strategiche (lockate in brainstorm)

1. **Scope smoke contract**: full A+B+C — (A) `block_on` sopravvive per init
   ma l'executor reale prende il posto del runtime, (B) almeno 2 task
   cooperativi schedulati interleaved, (C) future svegliata da una IRQ
   diversa dal timer (= keyboard ISR).
2. **Executor crate**: `embassy-executor` 0.6 con `default-features = false`,
   **no `arch-*` feature** → forniamo il nostro `__pender` (custom Pender).
   Idle = `hlt` nel kmain loop (CPU davvero ferma, risveglio via IRQ).
3. **kmain shape**: α "coexist" — `vfs::block_on` resta per init sync
   (VFS smoke al boot intatto); poi `EXECUTOR.run(|spawner| { … })`
   prende il controllo per sempre.
4. **Primitives**: hand-roll `Delay(ticks)` future + "replace" del keyboard
   ISR (la ISR pusha in coda invece di stampare direttamente). ~120 LoC
   nostri. embassy-time aggiunta opportunisticamente più avanti.

## Architettura

```
kmain()
 │
 ├── INIT (sync, codice esistente)
 │     heap → idt → paging → apic → vfs → fb → ansi
 │     vfs::block_on(boot_smoke())          // Step 7 helper, intatto
 │
 └── STEADY-STATE (executor, mai ritorna)
       EXECUTOR.run(|spawner| {
           spawner.spawn(tick_task()).unwrap();
           spawner.spawn(kbd_echo_task()).unwrap();
       })
       // ! - hlt-idle loop interno
```

Loop interno di `Executor::run` (semplificato):

```text
loop {
    poll_all_ready_tasks();
    if no_ready_tasks {
        wait_for_pender_signal();   // = hlt + IRQ wakes us
    }
}
```

Il nostro `__pender` non fa nulla: il segnale è implicito perché ogni
IRQ ritorna in `wait_for_pender_signal`, e prima di ri-hlt l'executor
rilegge la run queue.

## Componenti

### Nuovi file

#### `kernel/src/executor/mod.rs`
- `static EXECUTOR: embassy_executor::Executor` (init lazy o `const fn`)
- `extern "Rust" fn __pender(_: *mut ())` — no-op
- `pub fn run() -> !` — wrapper che chiama `EXECUTOR.run(|spawner| { spawn tutto })`
- Idle hook: `Executor::run` di embassy gestisce HLT internamente quando
  costruita con custom Pender + `wait_for_event` impl che fa `hlt`. Da
  verificare in implementazione; fallback = override del loop esterno.

#### `kernel/src/executor/delay.rs`
- `struct Delay { target_ticks: u64, slot: Option<usize> }`
- `impl Future for Delay`:
  - `poll`: se `timer::ticks() >= target_ticks` → `Poll::Ready(())`
  - altrimenti registra `cx.waker().clone()` in `DelayList`, salva slot, `Poll::Pending`
- `impl Drop for Delay`: se `slot.is_some()`, libera lo slot
- `pub fn ticks(n: u64) -> Delay` — costruttore (target = now + n)

`DelayList`:
```rust
struct DelayEntry { target_ticks: u64, waker: Waker }
static DELAY_LIST: spin::Mutex<[Option<DelayEntry>; 8]> = ...;
```
- Senders (task): `without_interrupts(|| DELAY_LIST.lock())` per evitare
  deadlock con ISR
- ISR: `DELAY_LIST.try_lock()` — se busy salta questo tick (max 10 ms
  ritardo, accettabile). Scan lineare, per ogni slot due: `take()` +
  `waker.wake()`

`pub fn timer_tick(now: u64)` chiamata da `timer::timer_handler` con
`TICKS.load()` corrente.

#### `kernel/src/keyboard/queue.rs`
(nuovo submodule, `keyboard.rs` diventa `keyboard/mod.rs`)

- `struct KbdQueue { buf: [u8; 64], head: usize, tail: usize, dropped: AtomicU64, waker: Option<Waker> }`
- Protezione: `spin::Mutex<KbdQueue>`; sender (ISR) e consumer (task)
  entrambi lockano. Task usa `without_interrupts`. ISR usa lock diretto
  (single-CPU, l'unico altro lock attore è il consumer task che disabilita IRQ).

  Alternativa più rigorosa: lock-free SPSC ring (head/tail atomici) + Waker
  in cella separata. Decisione in implementazione; partire dalla versione
  con Mutex (più semplice da review-are).

- `pub async fn read_char() -> u8` — async fn che restituisce il prossimo char dal queue, o aspetta

- `pub fn push_from_isr(c: u8)` — chiamata dalla ISR; pusha (con drop+counter se pieno) e fa `take()` del waker per chiamare `wake()` fuori dal lock

### File modificati

#### `kernel/Cargo.toml`
Aggiunta:
```toml
embassy-executor = { version = "0.6", default-features = false, features = ["nightly", "task-arena-size-4096"] }
```
Niente `arch-*` feature → forniamo `__pender` custom.

(Se la versione 0.6 ha API che non compone col nostro pender custom su
no_std/x86_64-bare-metal, fallback alla 0.5 dove l'API custom-pender è
documentata; implementer testa entrambe.)

#### `kernel/src/main.rs`
Dopo il blocco VFS smoke esistente, prima del `loop { hlt }` corrente:

```rust
kprintln!("ruos: executor up");
executor::run();   // ! - non ritorna
```

Il `loop { hlt }` finale di `kmain` muore (l'executor lo sostituisce).

#### `kernel/src/timer.rs`
`timer_handler` cambia da:
```rust
TICKS.fetch_add(1, Ordering::Relaxed);
crate::console::fb::tick_cursor();
lapic::eoi();
```
a:
```rust
let now = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
crate::console::fb::tick_cursor();
crate::executor::delay::timer_tick(now);
lapic::eoi();
```

#### `kernel/src/keyboard.rs` (→ split in `keyboard/mod.rs` + `keyboard/queue.rs`)
ISR cambia da "decode + kprintln" a "decode + queue::push_from_isr".
Niente più output dalla ISR.

### Task demo

In `kernel/src/main.rs` (o nuovo `kernel/src/tasks.rs`):

```rust
#[embassy_executor::task]
async fn tick_task() {
    let mut n: u32 = 0;
    loop {
        executor::delay::ticks(100).await;   // 1s @ 100Hz
        kprintln!("ruos: async tick={}", n);
        n = n.wrapping_add(1);
    }
}

#[embassy_executor::task]
async fn kbd_echo_task() {
    loop {
        let c = keyboard::queue::read_char().await;
        kprintln!("ruos: kbd echo={:?}", c as char);
    }
}
```

## Data flow

### Delay wake path (tick_task)

```
T0:  tick_task: Delay::ticks(100).await
       poll(): now=42, target=142 → register_waker(slot=0, 142, waker) → Pending
T1:  executor: nessun task ready → __pender → wait_for_event
       kmain (o embassy): hlt
T2:  [10 ms più tardi] timer IRQ fires
       TICKS = 43
       fb::tick_cursor()
       executor::delay::timer_tick(43): scan slots, 43<142 → noop
       eoi()
T3:  resume da hlt, executor loop: ancora pending → hlt
…    (100 volte)
T4:  TICKS = 142
       timer_tick(142): slot 0 due → waker.wake() → slot cleared
       eoi()
T5:  executor: tick_task pronto → poll() → Delay::Ready → kprintln → loop top
       nuova Delay::ticks(100) → registrato slot 0, target=242 → Pending
T6:  hlt nuovamente
…
```

### Keyboard wake path (kbd_echo_task)

```
T0:  kbd_echo_task: queue::read_char().await
       poll(): buf vuoto → store waker → Pending
T1:  hlt
T2:  user preme 'a' → IRQ 1
       PS/2 controller → scancode → 'a' = 0x61
       KbdQueue.push_from_isr(0x61):
         buf.push(0x61)
         waker = self.waker.take()
       (esce dal lock) waker.wake()
       eoi()
T3:  resume, executor polls kbd_echo_task → read_char Ready('a') → kprintln
```

## Error handling

| Errore | Reazione |
|--------|----------|
| `EXECUTOR.spawner.spawn(...)` ritorna Err | `unwrap()` = panic (slab statica, errore di compile-config) |
| `DelayList` 8 slot pieni | panic (al massimo ~3 task previsti, slot exhaustion = bug) |
| `KbdQueue` pieno (64 byte) | drop char + `KBD_DROPPED.fetch_add(1)`; nessun panic |
| `DELAY_LIST.try_lock` fallisce in ISR | salta questo tick; max 10 ms ritardo |
| Task panic | propaga a kernel panic (no recovery) |

## ISR safety

| Risorsa | ISR contesto | Task contesto | Strategia |
|---------|--------------|---------------|-----------|
| `TICKS` (AtomicU64) | `fetch_add(Relaxed)` | `load(Relaxed)` | Atomic, no lock |
| `DELAY_LIST` (Mutex<[…; 8]>) | `try_lock` (skip if busy) | `without_interrupts \|\| lock()` | Mutex + IRQ-disable side |
| `KbdQueue` (Mutex<…>) | `lock()` (sicuro: consumer disabilita IRQ) | `without_interrupts \|\| lock()` | Mutex + IRQ-disable side |
| Waker calls | `wake()`/`wake_by_ref()` from ISR | n/a | Embassy Waker è ISR-safe by design |

**Invariante kritico**: nessun task può tenere `DELAY_LIST.lock()` o
`KBD_QUEUE.lock()` con le interrupt abilitate. Pattern: sempre
`without_interrupts(|| { let g = lock(); … })`.

## Testing

### Boot smoke (run-test)

Log atteso al boot:
```
ruos: hello serial
... (init existing logs) ...
ruos: vfs smoke ok n=3 buf=[abc]
ruos: fb ok 1280x800 ...
ruos: fb attached
ruos: ansi test ok
ruos: executor up
ruos: async tick=0
ruos: async tick=1
ruos: async tick=2
```

`Makefile` cambia `HELLO`:
- Da: `HELLO := ruos: ticks=`
- A:   `HELLO := ruos: async tick=2`

Prova che almeno 3 tick async sono avvenuti = ~3 s di kernel time
= executor running + Delay future + timer-IRQ-wake all green.

### Manual smoke (`make run`)

Premere tasti in QEMU/VirtualBox: ogni char emette
`ruos: kbd echo='X'`. Prova il wake path della keyboard IRQ.

### Regression checks

- `make build` clean (warning solo pre-esistenti)
- VFS smoke ancora ok (block_on intatto)
- Cursor blink ancora visibile (timer_handler non perde
  `fb::tick_cursor()`)

## File toccati (riepilogo)

**Nuovi:**
- `kernel/src/executor/mod.rs`
- `kernel/src/executor/delay.rs`
- `kernel/src/keyboard/queue.rs`
- (rinominato `kernel/src/keyboard.rs` → `kernel/src/keyboard/mod.rs`)

**Modificati:**
- `kernel/Cargo.toml` (+1 dep)
- `kernel/src/main.rs` (executor::run sostituisce `loop { hlt }`)
- `kernel/src/timer.rs` (timer_tick chiamata)
- `kernel/src/keyboard/mod.rs` (ISR push invece di kprintln)
- `Makefile` (HELLO grep)

## Open points (decisi in implementazione, non blocking)

- **embassy-executor 0.6 vs 0.5**: testare entrambi col custom pender. Il
  symbol `__pender` è stabile dal 0.4; le features differiscono.
- **HLT path**: se embassy 0.6 ha un internal idle hook OK, sennò
  override del run loop con un wrapper nostro (poll + hlt manuale).
- **`KbdQueue` lock-free vs Mutex**: partire con Mutex, refactor a SPSC
  ring se diventa un bottleneck (improbabile in Step 9).
