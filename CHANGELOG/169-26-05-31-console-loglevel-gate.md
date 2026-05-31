# 169 — console loglevel gate: INFO post-boot solo in dmesg

**Data:** 2026-05-31

## Cosa
I log strutturati del kernel (`binfo!`/`bwarn!`/`berr!`) ora hanno una
soglia di livello per il **framebuffer** (lo schermo). Tre sink, due
sempre attivi e uno gated:

- **Ring buffer** (`dmesg`): riceve sempre ogni riga.
- **Seriale**: riceve sempre ogni riga — è il "filo" di debug/log, ed è
  ciò che gli script di test (`run-test`, `run-ssh-test`, ...) leggono.
- **Framebuffer** (console a schermo): riceve solo righe `>=` soglia.

Implementazione:
- `kernel/src/boot/log.rs`: `CONSOLE_LEVEL: AtomicU8` (INFO=0, WARN=1,
  ERR=2), `set_console_level`/`console_level`. `emit` formatta la riga
  una volta, la pusha sempre nel ring buffer, scrive sempre sul seriale
  e disegna sul framebuffer solo se il livello supera la soglia
  (`MultiConsole::write_serial_only` per il caso sotto-soglia).
- `kernel/src/console/mod.rs`: nuovo `MultiConsole::write_serial_only`.
- `kernel/src/boot/phases/userland.rs`: subito prima di
  `executor::run()` chiama `set_console_level(LEVEL_WARN)`. Da quel
  punto INFO non disegna più a schermo; WARN/ERR sì.

Default = INFO durante tutto il boot, così la sequenza completa resta
visibile sullo schermo. La transizione a WARN avviene all'handoff a
userland.

## Perché
Segnalazione utente: a OS avviato (es. `[T+1063.974s] INFO ...`) i log
del kernel continuavano a comparire sulla console a schermo, mescolati
con l'I/O della shell locale (framebuffer / finestra VirtualBox).
Comportamento corretto di un OS — come Linux con `printk` vs il console
loglevel — è che dopo il boot gli INFO finiscano nel ring buffer
(`dmesg`) e non sporchino la console interattiva. Eventi tipo "ssh
client connected/auth ok/session done" o il watchdog [[166-26-05-30-pty-watchdog]]
sono INFO/WARN: ora gli INFO sono silenziati a schermo, i WARN (problemi)
restano visibili.

Scelta di gate solo il framebuffer e non il seriale: il seriale è il
canale di log/debug (come un dmesg-over-wire) e gli asserts dei test ci
si appoggiano — silenziarlo avrebbe rotto `run-test` (es.
`dhcp bound ip=`) e `run-ssh-test` (`auth ok`), che sono INFO emessi a
runtime dopo l'avvio dell'executor.

## Note / follow-up possibili
- Soglia post-boot hardcoded a WARN. Un `dmesg -n <level>` (host fn per
  `set_console_level`) la renderebbe regolabile a runtime — TODO.
- `kprintln!` (path raw, usato per errori rari) resta sempre su console:
  non è stato gated, è volutamente "loud" per i casi d'errore.

## File toccati
- kernel/src/boot/log.rs
- kernel/src/console/mod.rs
- kernel/src/boot/phases/userland.rs
