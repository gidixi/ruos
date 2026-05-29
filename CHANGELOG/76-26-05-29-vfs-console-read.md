# 76 — Console::read wires keyboard queue

**Data:** 2026-05-29

## Cosa

`vfs::devices::ConsoleFile::read` rimpiazza il vecchio EOF stub con
`keyboard::queue::read_char().await`. Restituisce 1 byte per call (single
char), `0` se `buf.is_empty()`.

Doc commento avverte: keyboard queue è single-consumer; chiunque altro
legga concorrente-mente (oggi: `SuspendReason::KbdReadChar` via
`FdEntry::Stdin`) racera. Step 11 sceglie un single consumer (shell).

## Perché

Senza questo, `open("/dev/console") → read` ritorna sempre 0 (EOF).
Shell.wasm e altri tool che vogliono trattare console come file Unix
non potrebbero leggere stdin via il path "everything-is-a-file".

Step 11 shell sfrutterà /dev/console come FD 0 = stdin canonical;
KbdReadChar/Stdin path resta legacy/dual finché il refactor della
RuntimeState elimina la special-case.

## File toccati

- kernel/src/vfs/devices.rs
- CHANGELOG/76-26-05-29-vfs-console-read.md (nuovo)
