# Step 11 — followups

Followup emersi dal whole-implementation review di Step 11. Aperti al merge
di `feature/step-11-shell` → `main`. **Nessuno blocca Step 12**, ma F1/F2/F4
vanno affrontati prima dello Step 15 (SSH: serve secondo wasm interattivo).

## F1 — `EXEC_QUEUE` single-slot mpmc

**File:** `kernel/src/wasm/exec_queue.rs`
**Severity:** 🟠 important pre-Step-15

`EXEC_QUEUE` ha esattamente uno `pending`, uno `result`, uno `done`, un
`shell_waker`. Due fiber che chiamano `post_and_wait` concorrenti:
- secondo `*pending.lock() = Some(slot)` sovrascrive il primo prima che
  il worker dequeua → request silently dropped
- secondo waker overwrite il primo → only second woken

Oggi funziona perché solo `shell.wasm` chiama `ruos_exec`. Si rompe non
appena un secondo issuer esiste (es. shell SSH-served in Step 15).

**Fix:** `VecDeque<ExecSlot>` per `pending` + per-future done flags
(un `Arc<AtomicBool>` per request invece di un global `done`).

## F2 — `ExecFuture::poll` post-load-store-waker race

**File:** `kernel/src/wasm/exec_queue.rs::ExecFuture::poll`
**Severity:** 🟠 important pre-SMP

Sequenza:
1. post slot to queue
2. `done.load()` — false
3. register waker

Se worker gira tra step 2 e 3 → done=true, wake() called on empty waker,
poi waker registrato ma mai woken → miss-wake.

Single-CPU + cooperative oggi nasconde il bug (executor non può girare
tra due linee dello stesso poll). SMP o preemption futuri lo esporrebbero.

**Fix:** pattern compare-store-recheck: store waker prima, poi check
`done.load()` di nuovo; oppure store waker prima del post + atomic
swap.

## F3 — `ExecSlot.exit_code: *mut i32` dead field

**File:** `kernel/src/wasm/exec_queue.rs:23`
**Severity:** 🟡 cleanup

Field settato a `null_mut()` ovunque, "unused; result via AtomicI32".
Drop field + `unsafe impl Send/Sync` diventa non necessario.

## F4 — Keyboard queue single-Waker race

**File:** `kernel/src/keyboard/queue.rs:91`
**Severity:** 🟠 important pre-Step-15

`s.waker = Some(cx.waker().clone())`: secondo poller overwrite il primo.
Solo uno wakeato al prossimo char. Pre-esistente; F7 di Step 10.5 era
mitigato droppando kbd_echo_task. Riemerge appena due wasm leggono
stdin (es. shell SSH + shell locale).

**Fix:** `VecDeque<Waker>` di subscribers, o `AtomicBool SUBSCRIBED`
che asserta single reader.

## F5 — `path_open` ignora oflags (pre-esistente Step 10.5)

**File:** `kernel/src/wasm/host/path.rs:38`
**Severity:** 🔴 critical behavior bug, ma pre-esistente — non blocca Step 11

`path_open` hardcoded `OpenFlags::CREATE | WRITE | READ`. `cat /missing`
crea un file vuoto invece di ritornare ENOENT. Surface bug visibile da
shell.wasm via cat/echo redirect (futuro).

Già nei followup di Step 10.5 come F5. Riconfermato qui — ora ha
surface user-visible.

**Fix:** parse WASI oflags + fs_rights_base + fdflags. ~30 LoC.

## F6 — `limine.conf stack_size: 0x200000` defensive but redundant

**File:** `limine.conf:6`
**Severity:** 🟡 cleanup

Aggiunto in Step 11 T2 come precauzione contro lo stack overflow del
recursive `Box::pin(child.run())`. La fix reale = `exec_worker_task`.
Il bump del kernel boot stack di Limine non aiuta gli embassy task
stacks (per-pool, allocati dall'heap).

**Fix:** verificare se rimovibile (basta lo stack default 64 KB).
Tenere se serve mai per init-time deep stack (e.g. acpi/limine response
parsing — non oggi).

## F7 — `fd_filestat_get` ritorna size=0

**File:** `kernel/src/wasm/host/fd.rs:200-222`
**Severity:** 🟠 important quando arriveranno tool che pre-allocano

`std::fs::read_to_string` su shell.wasm funziona via read-loop fallback.
Ma futuro tool che fa `metadata().len()` e pre-alloca Vec con quella
size leggerà 0 → Vec piccolo → re-grow per ogni chunk. Performance hit
ma non correttness.

**Fix:** tracciare path per FD (estende `FdEntry::Vfs(Fd, PathBuf)`),
poi `vfs::stat(path)` per la dimensione. Riprovare per `cat` su file
grossi.

## F8 — Verbose `kprintln!` ad ogni suspend

**File:** `kernel/src/wasm/fiber.rs:80`
**Severity:** 🟡 noise

`kprintln!("ruos: wasm fiber: suspend {:?}", reason);` per ogni
suspend. `cat /bin/shell.wasm` dumpa 60+ righe di `VfsRead` traces.

**Fix:** gate behind `cfg(feature = "wasm-trace")`, o counter atomic
+ dump periodico, o downgrade a 1 line per fiber start.

## F9 — `decode_argv` silent failure → empty args + errno=0

**File:** `kernel/src/wasm/host/proc.rs:38-52`
**Severity:** 🟡 suggestion

`decode_argv` valida bene (`checked_add`/`checked_mul`) ma ritorna
`None` su malformed → `unwrap_or_default()` → empty argv + errno=0.
Guest non vede che il blob era malformato.

**Fix:** ritornare EINVAL (28) invece di success-with-empty.

## F10 — Pattern rule Makefile per user wasms

**File:** `Makefile:24-50`
**Severity:** 🟡 cleanup

7 rule quasi identiche per ogni user crate. Pattern rule
`user-bin/%.wasm: user/%/src/main.rs ...` ridurrebbe a 5 righe.

## F11 — `path_len` / `argv_len` non bounded check in host fns

**File:** `kernel/src/wasm/host/proc.rs:18-27`
**Severity:** 🟡 defensive

Buggy wasm può passare `i32::MAX` come len → tentativo alloc 2 GiB →
panic talc. Threat reale basso ma cap (4 KiB path, 64 KiB argv) è
gratis.

**Fix:** validate len < cap before alloc, return EINVAL se eccede.

## F12 — wasm_task pool_size = 4 stretto

**File:** `kernel/src/executor/mod.rs:54-58`
**Severity:** 🟡 future-compat

4 wasm task spawned (init/server/client/shell). pool_size = 4 →
qualunque 5° spawn panic. Aggiungere safety margin (es. 8).

## F13 — Dirent layout duplication tra kernel e ls.wasm

**File:** `kernel/src/wasm/fiber.rs:257-281` + `user/ls/src/main.rs:27-44`
**Severity:** 🟢 nit

Encoder kernel + decoder ls.wasm hanno byte layout duplicato. Drift
possibile.

**Fix:** crate condiviso `user/ruos-abi` con const layouts. Da
riconsiderare quando arriveranno più tool che usano dirent.

---

## ✅ CLOSED (2026-05-29)

- **F3** (embassy-futures unused) — chiuso da CHANGELOG/100.
- **F4** (keyboard queue single-Waker race) — chiuso da Step 12 T3.
- **F5** (path_open ignora oflags) — chiuso da CHANGELOG/100.
- **F7** (fd_filestat_get size=0) — chiuso da CHANGELOG/100 (new
  File::stat trait + vfs::stat_fd).
- **F8** (verbose suspend kprintln) — chiuso da boot polish
  (wasm-trace feature gate).
