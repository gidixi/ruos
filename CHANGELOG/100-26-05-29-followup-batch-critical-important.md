# 100 — Followup batch: critical + important fixes

**Data:** 2026-05-29

## Cosa

Batch di 5 fix per chiudere followup 🔴 critical + 🟠 important:

### F1 — path_open honors oflags + fs_rights_base (🔴 critical)
`kernel/src/wasm/host/path.rs`: deriva `OpenFlags` da WASI oflags
(`O_CREAT`/`O_TRUNC`) e `fs_rights_base` (`RIGHTS_FD_READ`/
`RIGHTS_FD_WRITE`). Default = READ se nessun right specificato.
Era hardcoded `CREATE|WRITE|READ` → `cat /missing` creava file.
Chiude **Step 10.5 F5** + **Step 11 F5**.

### F2 — ConsoleFile::read EOF stub (🟠 important)
`kernel/src/vfs/devices.rs`: ritorna `Ok(0)` invece di leggere da
`pty::master_output_read(0)` (workaround sbagliato T3 implementer:
era loop hazard, /dev/console legge propri output). Chiude
**Step 12 F1**.

### F3 — Termios ABI lock (🟠 important)
`kernel/src/pty/termios.rs`: `static_assert` `size_of::<Termios>() == 56`
+ align 4. Doc corregge "60-byte" → "56-byte". Cattura accidentali
field additions. Chiude **Step 12 F2**.

### F4 — `embassy-futures` dep drop (🟠 cleanup)
`kernel/Cargo.toml`: rimossa. Nessun uso post-Step 10.5 (solo doc
comment reference). Chiude **Step 11 F3**.

### F5 — fd_filestat_get real size (🟠 important)
`kernel/src/vfs/{file,devices,tmpfs,mod}.rs`: nuovo `File::stat()`
trait method + `vfs::stat_fd(fd)` public API. `fd_filestat_get`
peeked FileImpl per kind + size reali (size from `content.len()` per
Reg, 0 per Dir/Device).
- tmpfs::TmpfsFile::stat() = `(Reg|Dir, content.len()|0)`
- PtySlaveFile/NullFile/ZeroFile/ConsoleFile::stat() = `(Device, 0)`

`cat large_file` → ora pre-alloca Vec right-size, no slow re-grow.
Chiude **Step 11 F7**.

### Audit Step 8 F3 (chiuso de-facto)
ConsoleFile::write ancora locka SERIAL senza `without_interrupts`,
ma:
- Keyboard ISR non chiama più `kprintln!` (Step 11 T3 + Step 12 T3
  retired path)
- Timer ISR non touch CONSOLE/SERIAL (solo `fb::tick_cursor()` raw
  write_volatile + atomic counters)
- Nessun ISR lock contesa SERIAL → nessun deadlock possibile

F3 marcato closed in `docs/followups/step-8.md`.

## Perché

User richiesto: "analiziamo tutti i followup, critical/important ora".
6 item chiusi (5 fix + 1 audit) in unica branch. Build clean + sentinel
PASS.

## File toccati

- kernel/src/wasm/host/path.rs (F1)
- kernel/src/vfs/devices.rs (F2 + F5)
- kernel/src/pty/termios.rs (F3)
- kernel/Cargo.toml (F4)
- kernel/src/vfs/file.rs (F5 trait + dispatch)
- kernel/src/vfs/tmpfs.rs (F5 impl)
- kernel/src/vfs/mod.rs (F5 stat_fd)
- kernel/src/wasm/host/fd.rs (F5 fd_filestat_get real)
- CHANGELOG/100-26-05-29-followup-batch-critical-important.md (nuovo)

(Updates to followup .md files coming in separate commit.)
