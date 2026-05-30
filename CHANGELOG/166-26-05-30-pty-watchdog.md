# 166 — software watchdog: SIGHUP su PTY + idle timeout

**Data:** 2026-05-30

## Cosa
Aggiunto un meccanismo di shutdown per i PTY pair, in due livelli:

1. **Per-session SIGHUP** (livello SSH).
   - `kernel/src/pty/mod.rs`: nuovo flag `SHUTDOWN: [AtomicBool; NUM_PAIRS]`,
     `request_shutdown(idx)` che setta il flag e sveglia il `slave_waker`.
   - `kernel/src/vfs/devices.rs::PtySlaveFile::read`: dopo aver
     drenato tutto `slave_rx`, se il flag è settato ritorna `Ok(0)` (EOF)
     invece di parcheggiarsi in `Poll::Pending`.
   - `kernel/src/ssh/sunset_io.rs::run_session`: alla fine del bridge
     (anche in caso di disconnect brutto / socket chiuso), invoca
     `pty::request_shutdown(pty_idx)`.
   - Lo shell.wasm su quel PTY vede `read_byte()` ritornare `None`,
     `read_line_raw` ritorna `None`, esce dal loop main, il
     `ssh_pty_dispatcher_task` rilascia il pair → slot libero per la
     prossima connessione.

2. **Idle watchdog** (livello kernel).
   - `kernel/src/pty/mod.rs`: nuovo `LAST_ACTIVITY: [AtomicU64;
     NUM_PAIRS]` con timestamp tick (100 Hz). Touched da
     `master_input_push`, `master_output_try`, `PtySlaveFile::read`
     (quando ritorna byte > 0) e `PtySlaveFile::write`.
   - `kernel/src/executor/mod.rs`: nuovo `pty_watchdog_task` (embassy
     task). Ogni 10 s ispeziona i pair `1..NUM_PAIRS` (skippa il pair 0
     del boot shell): se `is_claimed(idx) && !is_shutdown(idx) && now -
     last_activity > 5 min`, chiama `request_shutdown(idx)`.

3. `release(idx)` ora resetta sia `SHUTDOWN` sia `LAST_ACTIVITY`, così
   un pair appena reclaimato parte pulito.

4. Cosmetico: tag dei log di `rebind_stdio_pty` da `"ssh"` a `"wasm"`,
   coerente con il fatto che il rebind è invocato anche dall'exec_worker
   (Step 16 follow-up [[164-26-05-30-exec-inherit-pty]]), non solo
   dall'SSH dispatcher.

## Perché
Hang osservato: se un client SSH si disconnette senza digitare `exit`,
il bridge `run_session` termina, ma lo shell.wasm spawnato sul PTY
resta in `read()` sul slave_rx — nessuno lo svglia, nessuno chiude il
socket dal lato dello shell. Risultato: il PTY pair resta `is_claimed
== true` per sempre. Dopo 3 disconnect bruti tutti i pair 1..3 sono
leaked e l'SSH server non riesce più a spawnare shell. Il pattern Unix
equivalente è il SIGHUP propagato dal terminale al processo group: qui
lo facciamo via EOF su stdin.

L'idle watchdog è il safety net sotto al SIGHUP per i casi in cui il
bridge stesso fallisce o un bug futuro lo bypassa.

## Note di scope
- Watchdog scope: solo PTY pair lifecycle. Non implementa fuel/gas
  limit sul codice wasm (un comando in loop puro che non yielda
  blocca comunque l'executor cooperativo single-thread — è un
  problema diverso, scope `wasmi::ResumableInvocation` o engine
  fuel).
- Boot shell (pair 0) escluso per design: nessun concetto di "session
  end", l'utente locale può lasciare il prompt idle.
- Threshold 5 min hardcoded; rendere configurabile via env build-time
  o `/mnt/conf` è TODO follow-up.

## Test
- `make run-test` → TEST_PASS (smoke battery)
- `make run-ssh-test` → TEST_PASS_SSH (pubkey + interactive)
- `make run-passwd-test` → TEST_PASS_PASSWD
- `make run-passwd-diskless-test` → TEST_PASS_PASSWD_DISKLESS

Un test dedicato per il SIGHUP (kill abrupto del client SSH +
verifica release pair entro N secondi) è TODO.

## File toccati
- kernel/src/pty/mod.rs
- kernel/src/vfs/devices.rs
- kernel/src/ssh/sunset_io.rs
- kernel/src/executor/mod.rs
- kernel/src/wasm/fiber.rs
