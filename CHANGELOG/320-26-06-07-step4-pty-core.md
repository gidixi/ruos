# 320 — SMP Step 4 (pty-core): PTY ownership, SPSC slave-input ring, owner-routed stdout

**Data:** 2026-06-07

## Cosa

Rimosso l'ultimo lock RW-hot cross-core sul data path interattivo del PTY. Prima
ogni operazione PTY prendeva `PAIRS[i]: Mutex<PtyPair>` con `without_interrupts`
(IF=0); sotto app `.cwasm` parallele, un'app su core 2 che scriveva stdout
contendeva quel lock cross-core con la master-read SSH del BSP.

Ora ogni pair ha un **owner core** (= BSP, core 0, per v1: `pty_owner(idx) -> 0`):

- **`slave_rx` → SPSC byte ring lock-free** (`kernel/src/pty/spsc.rs`,
  capacità 4096, power-of-two, head/tail `AtomicUsize`, alloc-free → safe in ISR).
  Producer unico = l'owner (line discipline); consumer unico = l'app-core che
  legge stdin. Sostituisce il vecchio `VecDeque` sotto lock. Array per-pair
  `SLAVE_RX` in `pty/mod.rs`.
- **stdout dell'app (direzione ad alto volume) → instradato all'owner via il bus
  u64 esistente** (`smp::inbox::request`), **un messaggio per `write()`** (NON
  per byte → nessuna regressione di throughput). Off-owner: `PtySlaveFile::write`
  chiama `route_write_to_owner(idx, buf)`; l'owner esegue `pty_write_op` (plain
  `fn`, solo pair lock) e fa `process_output` localmente nel suo `master_out`.
  Sull'owner (e su 1 core, dove `cpu_id() == owner` sempre) resta il fast path
  locale = comportamento odierno.
- **`foreground_pid` → `AtomicU32`** (`FOREGROUND[]`, 0 = nessuno): il read path
  app-side controlla kill/EOF senza prendere il pair lock.
- **slave waker → slot cross-core** (`SLAVE_WAKER[]: IrqMutex<Option<Waker>>`),
  fuori dal pair lock: il consumer registra (`register_slave_waker`) prima del
  re-check finale di vuoto; il producer sveglia (`wake_slave`) dopo `push` +
  Release (no lost wakeup). `master_input_push` e `request_shutdown` chiamano
  `wake_slave` dopo aver rilasciato il pair lock (ordine pair↔registry invariato:
  `request_kill` sempre dopo il drop del lock).
- **`master_out`, `termios`, `line_buffer`, `master_waker` → owner-local**:
  `master_out` ha due producer (stdout app via `process_output` + echo via
  `process_input`) quindi non può essere un SPSC pulito → resta owner-local, mai
  conteso cross-core. Rimossi da `PtyPair` i campi morti `master_in`/`slave_tx`
  (mai usati) oltre a `slave_rx`/`slave_waker`/`foreground_pid` (migrati).

**Risultato:** un app-core non prende MAI `pair(n)`. Le letture vengono dal ring
lock-free; le scritture vanno sul bus all'owner. Zero lock PTY cross-core → meno
jitter da interrupt (niente `without_interrupts` su un lock conteso cross-core),
di cui beneficia anche il GUI core; pronto per workload interattivi multi-app.

## Perché

Coerenza shared-nothing: il PTY era l'ultimo lock di data-path conteso
cross-core. Spec §9 (SMP shared-nothing). Step 4 della catena foundational.

## Verifica (raw markers)

Boot-check `-smp 4`:
- `pty-route from=core2 routed_ok=true spawned=true` (write off-owner instradata
  all'owner via bus e processata lì)
- `parallel-exec ran=[2,3] concurrent_ms=2940 single_ms=2890 overlap=true` (path
  exec invariato)

Suite di regressione:
- `TEST_PASS` (run-test, shell stdio su 1 core)
- `EXEC_AP_OK` + `exec-ap ran_on=core2 code=0` + `TEST_PASS_EXEC_AP`
  (.cwasm su core 2, stdout al terminale via il routed write)
- `TEST_PASS_SSH`, `TEST_PASS_SMP`, `TEST_PASS_SMP2`

Nessun `#DF`/`#PF`/panic.

## File toccati

- kernel/src/pty/spsc.rs (nuovo)
- kernel/src/pty/mod.rs
- kernel/src/pty/pair.rs
- kernel/src/pty/ldisc.rs
- kernel/src/vfs/devices.rs
- kernel/src/executor/mod.rs
- kernel/src/boot/phases/interrupts.rs
- CHANGELOG/320-26-06-07-step4-pty-core.md
