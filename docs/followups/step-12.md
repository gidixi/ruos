# Step 12 — followups

Followup emersi durante implementazione PTY. Aperti al merge di
`feature/step-12-pty` → `main`. Nessuno blocca merge; F1/F2 da fixare
prima dello Step 15 (SSH).

## F1 — `ConsoleFile::read` semanticamente wrong

**File:** `kernel/src/vfs/devices.rs::ConsoleFile::read`
**Severity:** 🟠 important pre-Step-15

T3 implementer cambiato `ConsoleFile::read` per usare
`pty::master_output_read(0)` come workaround dopo aver rimosso
`keyboard::queue`. Ma master_output_read legge bytes DESTINATI al
framebuffer (shell stdout). Se qualcuno apre `/dev/console` e legge,
riceve i propri output. Loop infinito potenziale.

Oggi nessuno apre `/dev/console` (shell usa `/dev/pts/0`). Dead path.

**Fix:** ridirezionare `ConsoleFile::read` a leggere da `pty[0].slave_rx`
(stessa source di `/dev/pts/0`), o stub EOF (`Ok(0)`). Step 14+ deciderà
semantica esatta di `/dev/console`.

## F2 — Termios layout assunto, non verificato vs wasi-libc

**File:** `kernel/src/pty/termios.rs` + `user/shell/src/main.rs`
**Severity:** 🟠 important pre-Step-15

Termios 60-byte struct (4xu32 + 32xu8 + 2xu32) basata su assunzione
matching wasi-libc `__wasi_termios_t`. shell.wasm definisce stesso
layout in extern "C" per pattern matching. Se wasi-libc dovesse cambiare
ABI o se altri tool wasm (es. python.wasm in futuro) usassero layout
diverso → tcgetattr/tcsetattr corrompono memoria wasm.

**Fix:** verificare con `wasm-objdump -x` su un binario wasm32-wasip1
che usa termios. Aggiungere `static_assert` sulla size = 60 in
`kernel/src/pty/termios.rs`. Documentare in `docs/wasix-abi-snapshot.md`.

## F3 — `/dev/ptmx` dynamic allocator mancante

**File:** `kernel/src/pty/` (future)
**Severity:** 🟡 pre-Step-15

Spec Step 12 ha deliberatamente skip `/dev/ptmx` (Opt 2 brainstorm =
pool statico 4 pair). SSH (Step 15) richiederà per-connection PTY pair
allocation. Multiple opzioni:
- (a) Pair index incrementale fino a NUM_PAIRS poi error
- (b) PtyPair allocato dinamicamente, slave entry creato/distrutto in
  VFS al volo
- (c) Aumentare NUM_PAIRS a 32 e usare pool con free-list

**Fix:** decidere quando arriverà Step 15. Probabilmente (c) → (b) se
serve scalabilità.

## F4 — Interactive line editor non testato programmaticamente

**File:** `user/shell/src/main.rs`
**Severity:** 🟡 doc / followup

Line editor (arrow keys, history, tab, Ctrl-A/E/L/C) implementato ma
non verificabile via `make run-test` (no stdin). Necessita test manuale
in VBox/QEMU interactive.

**Fix:** scrivere QEMU test harness che usa `-monitor stdio` + `sendkey`
per inviare keystroke. Step 13+ quando arriveranno più test interattivi.

## F5 — Multi-iov fd_read/fd_write su PTY EINVAL

**File:** `kernel/src/wasm/host/fd.rs`
**Severity:** 🟡 carry-over Step 10

PtySlaveFile read/write supporta solo single-iov (heritage Step 10
multi-iov EINVAL guard). `print!`/`println!` di shell.wasm sono
single-iov → funzionano. Tool wasm che usa `writev` → fallisce.

**Fix:** estendere fd_read/fd_write per multi-iov verso PTY (e VFS in
generale). Già in `docs/followups/step-11.md::F6`.

## F6 — `pty::master_input_push` mutex contention da ISR

**File:** `kernel/src/pty/mod.rs::master_input_push`
**Severity:** 🟢 nit

ISR locka `PAIRS[idx]` ogni keystroke. Single-CPU = no contention real,
ma se in futuro shell-side task tiene il lock (es. tcsetattr lungo)
mentre arriva tasto → ISR spinna. Non critical oggi.

**Fix:** considerare lock-free SPSC queue per master_in + waker
separato. Quando workload tastiera diventerà alto.

## F7 — History non persiste

**File:** `user/shell/src/main.rs::HISTORY`
**Severity:** 🟡 quality of life

History in-memory, persa al exit. Reboot azzera. Step 12.5/Step 14
potrebbe persistere su VFS (`/var/.shell_history`).
