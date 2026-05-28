# Step 10.5 — followups

Followup emersi dal whole-implementation review di Step 10.5 (WASIX
fibers). Aperti al merge di `feature/wasix-fibers` → `main`. **Nessuno
blocca lo Step 11**. Affrontati opportunisticamente o quando i file
sottostanti vengono toccati.

## F1 — Host fns `sock_open/bind/listen/connect` mancanti

**File:** `kernel/src/wasm/host/sock.rs`, `kernel/src/wasm/mod.rs::run_at`
**Severity:** 🟠 architecturally significant

Spec/plan Step 10.5 prometteva tutte le sock_* migrate a SuspendReason.
Solo `sock_accept` + `sock_connect` sono linkate. `sock_open`/`bind`/
`listen` non esistono come host fns; il kernel pre-alloca lato server
una listening socket su FD 4 e lato client una connected socket prima
che il fiber parta.

Data exchange è cooperative reale (ping/pong via `fd_read`/`fd_write`
→ SuspendReason → smoltcp). Ma il **setup** della socket è ancora
kernel-mediated. La asserzione "no preload" vale per i bytes, non per
la socket establishment.

**Fix:**
1. Implementare `sock_open`/`sock_bind`/`sock_listen` come host fns
   sync (operazioni instant su smoltcp, no future).
2. Implementare `sock_connect` come trap SuspendReason (già fatto).
3. Modificare `server.wasm`/`client.wasm` per chiamare le sequenze
   POSIX-style (`sock_open` → `sock_bind` → `sock_listen` → `sock_accept`).
4. Rimuovere il pre-allocate in `wasm/mod.rs::run_at`.
5. Aggiornare doc del modulo `host/sock.rs` (oggi claim'a sock_open/bind/
   listen "instant" ma non esistono).

## F2 — `sock_accept` ritorna stesso FD di listen

**File:** `kernel/src/wasm/fiber.rs::dispatch(SockAccept)`
**Severity:** 🟠 non-conforme WASI

smoltcp single-socket listen→established model: `accept` non alloca
nuova socket, la listening socket stessa passa a Established. Fiber
dispatch scrive `cur_fd` come "new_fd" tramite `find_fd_for_handle`.

Conseguenza: server con multiple connessioni concorrenti non
funziona (la listening socket è ora "accepted"; nessuno listena più).

**Fix:** quando `sock_accept` succeede:
1. Alloca una nuova smoltcp socket pinned al Listen state sullo
   stesso port.
2. Trasferisci la connessione corrente alla nuova socket.
3. Ritorna il FD della nuova socket; la "original" listen socket
   resta in Listen.

Da rivisitare quando arriverà un server multi-client (Step 13+ rlvgl
remote? Step 15 SSH?).

## F3 — `embassy-futures` dep unused

**File:** `kernel/Cargo.toml`
**Severity:** 🟠 cleanup

Post-T3 zero `block_on` chiamate da host fns. Dep `embassy-futures = "0.1"`
resta in Cargo.toml senza usi. Rimuovere.

## F4 — `vfs::VfsError` → wasi_errno mapping mancante

**File:** `kernel/src/wasm/fiber.rs` (varianti Vfs in dispatch)
**Severity:** 🟡 future-compat

Tutte le error path Vfs ritornano errno=8 (EBADF) o 44 (ENOENT)
indipendentemente dalla causa reale. App che distinguono EIO/EISDIR/
ENOTDIR/EPERM vedono errno fittizio.

**Fix:** funzione `vfs_err_to_wasi_errno(VfsError) -> i32` che mappa
ogni variante. ~15 LoC.

## F5 — `path_open` ignora oflags / fs_rights / fd_flags

**File:** `kernel/src/wasm/host/path.rs`
**Severity:** 🟡 future-compat

Hardcoded `OpenFlags::CREATE | WRITE | READ`. Un wasm che fa
`open("/etc/passwd", O_RDONLY)` ottiene handle writable+create.

**Fix:** parse oflags WASI:
- `O_RDONLY` → `OpenFlags::READ`
- `O_WRONLY` → `OpenFlags::WRITE`
- `O_RDWR` → `OpenFlags::READ | WRITE`
- `O_CREAT` (1) → add CREATE
- `O_TRUNC` (2) → add TRUNCATE (se VFS lo supporta)

## F6 — Multi-iov per Socket/Vfs/Stdin

**File:** `kernel/src/wasm/host/fd.rs`
**Severity:** 🟡 future-compat

`fd_read`/`fd_write` ritornano EINVAL per `iovs_len != 1` su Socket/
Vfs/Stdin paths. Demo single-iov: OK. `bash.wasm` userà `readv`/
`writev` di sicuro.

**Fix:** due opzioni:
- A: SuspendReason includono `Vec<(buf_ptr, len)>` per multi-iov in
  un solo trap. Una sola smoltcp/VFS call con buffer concat.
- B: trap-resume per iov: il loop esterno itera, traps per ciascun
  iov, accumula totale. Più semplice ma più overhead.

## F7 — Race `KbdReadChar` vs `kbd_echo_task`

**File:** `kernel/src/wasm/fiber.rs::dispatch(KbdReadChar)`
**Severity:** 🟠 da risolvere prima dello Step 11

Sia il dispatch fiber che `kbd_echo_task` consumano da
`keyboard::queue::read_char()`. Single-consumer queue: una read viene
ricevuta da uno solo. Cardine non-deterministica chi vince.

Pre-esistente da Step 10 ma ora reachable da wasm via WASIX
`fd_read(stdin)`. Step 11 (shell) avrà bisogno di "exclusive
keyboard ownership" per la shell.

**Fix preliminare**: drop `kbd_echo_task` o farlo conditional. Step
11 lo decide.

## F8 — Spec flow diagram non corrisponde all'as-built

**File:** `docs/superpowers/specs/2026-05-28-rust-wasix-fibers-design.md`
**Severity:** 🟡 doc drift

Sezione "Diagramma flow Task 2" mostra `client.wasm sock_connect` →
SYN. As-built: kernel pre-connetta lato client. Aggiornare diagram
per riflettere socket-activation model (= setup pre-fab, data flow
cooperative).

## F9 — Plan menziona `ResumableInvocation` invece di `ResumableCall`

**File:** `docs/superpowers/plans/2026-05-28-rust-wasix-fibers.md`
**Severity:** 🟢 nit (storico)

Plan scritto sotto assunzione che il type wasmi fosse
`ResumableInvocation`. Real wasmi 1.0.9 = `ResumableCall`.
Implementer ha adattato; cambia changelog 70 documenta. Plan
formal-non-aggiornato; lasciare così (plan = artefatto storico).

## F10 — Comment offset `clock_id` errato

**File:** `kernel/src/wasm/host/lifecycle.rs:80-84` (circa)
**Severity:** 🟢 nit

Comment dice `clock_id (u32)` a offset 16..24. Real layout:
- 16..20: clock_id (u32)
- 20..24: padding
- 24..32: timeout u64

Code corretto, comment errato. Fix una riga.

## F11 — `poll_oneoff` event userdata = 0

**File:** `kernel/src/wasm/fiber.rs::dispatch(Sleep)`
**Severity:** 🟢 nit

WASI `__wasi_event_t.userdata` (offset 0..8) deve essere copiato da
`subscription.userdata`. Oggi lasciato a 0. Per single-sub
(`thread::sleep`) il consumer non lo controlla. Per multi-sub
(quando arriverà) userdata serve per identificare quale sub ha
fired. Da estendere quando `poll_oneoff` riceverà più di una sub.
