# 307 — SMP Step 2: inter-core message bus + cross-core wake

**Data:** 2026-06-06

## Cosa

Implementazione completa dello Step 2 del piano SMP shared-nothing:

1. **Targeted IPI** (`lapic.rs`): `send_ipi(lapic_id, vector)` — IPI fisico a
   singolo core (physical destination mode, no shorthand, dest in ICR_HIGH[31:24]).

2. **VEC_INBOX + handler** (`idt.rs`): vettore `0x41` per inbox delivery IPI;
   handler `inbox_handler` → `mark_pending(cpu_id()) + eoi`. Riservati
   `VEC_TLB_SHOOTDOWN=0x42` e `VEC_RESET=0x43` come costanti documentate.

3. **Per-core inbox** (`smp/inbox.rs` — nuovo): `InboxMsg`, `ReplySlot` (Arc),
   `PER_CORE_INBOX: [CoreInbox; MAX_CPUS]`, `enqueue`, `request`, `drain_inbox`,
   `is_pending`, `ReplyFuture` (async request/reply con Waker cross-core).

4. **`lapic_id_of(cpu)`** (`cpu/mod.rs`): helper per encoded destination IPIs.

5. **Cross-core `__pender`** (`executor/mod.rs`): `WAKE_PENDING` promosso a
   `[AtomicBool; MAX_CPUS]`; `__pender` legge l'owner id dal context pointer e
   chiama `wake_core(owner)` (set flag + IPI se cross-core). BSP executor creato
   con context=0. Loop BSP drena inbox post-poll; halt condizionato a `is_pending`.

6. **AP inbox drain** (`cpu/ap.rs`): `ap_worker_loop` drena inbox dopo il pool;
   condizione halt include `!is_pending`.

7. **Boot-check round-trip** (`boot/phases/interrupts.rs`): BSP→AP1 message
   `sum([1,2,3,4])=10` con future poll inline. Con 1 core stampa "skipped".

## Perché

Fondamenta necessarie per Steps 3/4/5: cross-core wake ownership, async
request/reply tra core, budget vettori IPI documentato. Il test round-trip
è il gate di accettazione: dimostra IPI delivery, drain AP, complete reply,
future resolution sul BSP.

## File toccati

- `kernel/src/apic/lapic.rs`
- `kernel/src/idt.rs`
- `kernel/src/smp/inbox.rs` (nuovo)
- `kernel/src/smp/mod.rs`
- `kernel/src/cpu/mod.rs`
- `kernel/src/executor/mod.rs`
- `kernel/src/cpu/ap.rs`
- `kernel/src/boot/phases/interrupts.rs`
