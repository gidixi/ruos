# 311 — SMP Step 3c: cross-core spawn via embassy SendSpawner

**Data:** 2026-06-06

## Cosa

Implementato `spawn_on(cpu, token)` in `kernel/src/executor/mod.rs`:

- `PER_CORE_SPAWNER: [IrqMutex<Option<SendSpawner>>; MAX_CPUS]` — array statico
  di slot per-core, inizialmente `None`.
- `run_core(cpu)` pubblica `spawner.make_send()` in `PER_CORE_SPAWNER[cpu]` subito
  dopo aver costruito l'executor, prima del primo `hlt`.
- `pub fn spawn_on<S: Send>(cpu, token)` — lock il slot del core target, chiama
  `SendSpawner::spawn(token)`. Se il core non ha ancora pubblicato il suo spawner,
  ritorna `Err(SpawnError::Busy)` (il chiamante può riprovare).

Aggiunto boot-check Step 3c in `kernel/src/boot/phases/interrupts.rs`:

- `SPAWN_RAN_ON: AtomicU32` — registra su quale core gira la probe task.
- `cross_spawn_probe()` — task `#[embassy_executor::task]` che scrive
  `cpu_id()` in `SPAWN_RAN_ON`. Pubblica solo sotto `boot-checks`.
- Il BSP chiama `spawn_on(1, cross_spawn_probe())` con retry fino a 1 M iterazioni,
  poi attende fino a 50 M iterazioni che `SPAWN_RAN_ON != u32::MAX`.
- Log: `cross-spawn ran_on=core{} (spawned={}, expect core1)`.

### Risultato gate (-smp 4, boot-checks, 2 run):

```
run 1
[T+0.030s] INFO smp  3/3 APs online
[T+0.238s] INFO exec cross-spawn ran_on=core1 (spawned=true, expect core1)
run 2
[T+0.031s] INFO smp  3/3 APs online
[T+0.239s] INFO exec cross-spawn ran_on=core1 (spawned=true, expect core1)
```

`ran_on=core1` su entrambi i run: la task spawned dal BSP ha girato su core 1,
dimostrando la catena end-to-end:

`spawn_on(1)` → embassy enqueue atomico sul run-queue di core 1 → `__pender(1)`
→ `wake_core(1)` → IPI VEC_WAKE → core 1 esce da `hlt` → poll → task eseguita.

## Perché

Completa il fabric spawn/wake SMP: il BSP (o qualsiasi core) può ora iniettare
task su un core specifico in modo type-safe usando l'API `SendSpawner` di embassy
(queue atomica intrinseca, nessun tipo-erasure a mano). Step 5 (compositor su core
GPU dedicato) e la distribuzione di app WASM across cores usano questo primitivo.
Esercita per la prima volta la catena Step-2 cross-core wake in modo reale: il
round-trip Step-2 inbox usava un waker noop + poll inline; qui core 1 è davvero in
`hlt` e viene svegliato dall'IPI.

## File toccati

- `kernel/src/executor/mod.rs`
- `kernel/src/boot/phases/interrupts.rs`
- `CHANGELOG/311-26-06-06-smp-step3c-cross-core-spawn.md`
