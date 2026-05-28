# 48 — Console trait + MultiConsole + kprintln refactor (Step 8 Task 2)

**Data:** 2026-05-28

## Cosa
- `kernel/src/console/mod.rs` (riscritto): trait `Console`, struct
  `MultiConsole { serial, fb: Option<_> }`, impl `fmt::Write` per fan-out,
  static globale `CONSOLE: spin::Mutex<MultiConsole>` const-construttibile.
- `kernel/src/console/serial_con.rs` (nuovo): `SerialConsole` forwarder
  delegante a `SERIAL.lock().write_str(s)`.
- `kernel/src/kprint.rs`: `kprintln!` ora scrive su `CONSOLE.lock()` invece
  che su `SERIAL.lock()`. Behavior immutato finché FramebufferConsole non
  è attaccato (Task 3).
- `FramebufferConsole` ha ora un `impl Console` che delega ai metodi
  inerenti già esistenti.

## Perché
Secondo pezzo dello Step 8: abstraction layer per fan-out logging senza
ancora coinvolgere il framebuffer attivo.

## File toccati
- kernel/src/console/mod.rs
- kernel/src/console/serial_con.rs (nuovo)
- kernel/src/kprint.rs
- CHANGELOG/48-26-05-28-console-trait.md
