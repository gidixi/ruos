# 468 — poll-key hlt + kill cooperativo TUI; FIX race tabella FD globale

**Data:** 2026-06-11

## Cosa

Tre interventi legati, su rtop component (CHANGELOG 427):

1. **`poll_key` dorme invece di spinnare** (`wasm/wt/component.rs`): il loop
   di attesa tasto ora fa `sti; hlt` — il LAPIC timer 100 Hz dell'AP lo
   sveglia ogni 10 ms per ricontrollare ring/deadline/kill. Misurato in QEMU:
   processo qemu ~5-6% CPU con rtop idle (prima: una vCPU inchiodata al 100%
   per tutta la sessione).
2. **Kill cooperativo per i componenti TUI**: `exec_cwasm_parallel`
   (`wasm/fiber.rs`) ora fa `set_foreground(pts, pid)` come l'exec_worker
   wasmi; `poll_key` controlla `is_kill_pending(foreground)` e ritorna EOF —
   l'app smonta il terminale da sola ed esce. `pkill rtop` / `kill <pid>` da
   un'altra shell ora funzionano (anticipo del punto 5 della spec SP1).
3. **FIX bug pre-esistente — collisione della tabella FD globale**
   (`vfs/fd.rs`, `vfs/mod.rs`): `with_fd_take` SVUOTAVA lo slot per la durata
   dell'op async; una read bloccante (la shell console su stdin, parcheggiata
   lì il 99% del tempo) lasciava il suo numero fd riusabile da qualsiasi
   `open()` concorrente. Il runner TUI apre `/dev/pts/N` a ogni run → rubava
   sistematicamente lo slot della console: al tasto successivo il restore
   droppava il file pts0 della console, il cui stdin finiva cross-wired su
   pts1 — la console mangiava l'input della sessione SSH ('q' mai arrivata a
   rtop, tastiera console "morta"). Fix: slot tri-stato
   `Free / InFlight / Open` — `allocate()` non riusa mai slot InFlight;
   `close()` durante l'op mantiene la vecchia semantica (restore droppa).
   Latente da Step 10.5: colpiva in teoria ogni `.cwasm` (run_cwasm apre il
   pty stdout) con la console al prompt.

Test-infra: `tests/rtop-ssh-test.sh` (già in 427); script ad-hoc
`build/kill-test-adhoc.sh` — `pkill rtop` digitato sulla console locale via
QEMU monitor sendkey (il server SSH accetta una sessione sola), PASS.

## Verifica

- `make run-test` PASS; `tests/rtop-ssh-test.sh` 3/3 PASS (4 frame, q, prompt).
- Kill test console→SSH: alt-screen leave + prompt dopo `pkill rtop`; PASS.
- Tracce diag (rimosse): q consegnata a rtop in 8 ms; tastiera console viva
  durante rtop; nessuna lettura cross-pty.
- qemu ~5.5% CPU con rtop idle (hlt).

## File toccati

- kernel/src/wasm/wt/component.rs (poll_key hlt + kill check)
- kernel/src/wasm/fiber.rs (set_foreground su exec parallelo)
- kernel/src/vfs/fd.rs (Slot tri-stato + take/restore)
- kernel/src/vfs/mod.rs (with_fd_take su take/restore nuovi)
- kernel/src/wasm/host/term.rs (fd_to_pty → fd::pts_index)
- tests/rtop-ssh-test.sh (kill orfano qemu, -serial file:, -smp 4, -m 1024 — da 427)
