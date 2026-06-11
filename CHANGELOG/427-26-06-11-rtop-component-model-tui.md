# 427 — rtop su Component Model: tui.cwasm condiviso + dynamic linking kernel

**Data:** 2026-06-11

## Cosa

Completato lo split di rtop sul WebAssembly Component Model (`ruos:tui`):

- **`wit/ruos-tui.wit` riscritta**: interfaccia `canvas` **senza resource**
  (init/clear/draw-text/draw-bar/draw-table/`flush -> string`) + nuova
  interfaccia `host` (blob cpustat/proc-stat/meminfo, uptime, poll-key,
  set-raw, write-tty); worlds `tui-provider`, `tui-app` (export
  `run(once) -> s32`), `tui-host` (helper bindgen kernel).
- **`tools/wt-tui` riscritto**: i widget accumulano in UN back buffer ratatui
  (prima ogni draw-call cancellava la precedente); `flush()` fa diff vs
  l'ultimo frame e **ritorna** la stringa ANSI (niente `std::io`, rotto su
  wasm32-unknown-unknown).
- **`user/rtop` rifatto come componente puro**: cdylib wasm32-unknown-unknown,
  niente WASI né import raw "ruos" (così `wasm-tools component new` passa
  senza adapter); parser blob invariati in `sys.rs` (4 unit test host ok);
  `main.rs`/`raw.rs` rimossi; raw-mode/alt-screen via host.set-raw +
  write-tty; cursor-park `\x1b[24;80H` a fine frame (marker per i test).
- **Kernel `run_tui_component`** (`wasm/wt/component.rs`): deserializza
  `tui.cwasm` + app nello stesso Store; estrae le TypedFunc del provider via
  `get_export_index`; shim `func_wrap` su `ruos:tui/canvas` che forwardano al
  provider (StoreContextMut → call); `host` via bindgen world `tui-host`
  (blob da `sysinfo.rs` rifattorizzata in `*_blob()` condivisi, poll-key
  spin-wait su core AP / EOF su BSP, set-raw su termios PTY, write-tty su
  /dev/pts/N). Teardown ripristina sempre il termios.
- **Router**: `run_cwasm` riconosce gli artefatti componente
  (`Engine::detect_precompiled`) e li instrada al runner TUI caricando
  `/bin/tui.cwasm` dalla VFS; i core-module proseguono sul path WASI.
- **Makefile**: rtop buildato unknown-unknown → component → AOT; tolto dal
  giro wasip1.
- **`tests/rtop-ssh-test.sh` aggiornato**: `-smp 4` (interattivo richiede un
  core ComputeApp), `-m 1024` (heap kernel 384 MiB), `-serial file:` (il
  redirect bufferizzato perdeva il log al kill), kill dell'orfano qemu
  (uccidere `timeout` lasciava qemu vivo sulla porta 2222 → flakiness),
  conteggio frame via cursor-park, prompt case-insensitive, idle 6 s.

## Perché

`rtop` portava ratatui staticamente in ogni build wasip1; il provider
`tui.cwasm` condiviso gira AOT quasi-nativo e può servire altri tool TUI.
Primo dynamic linking componente↔componente del kernel (pattern: provider
istanziato nello stesso store + shim host sulle import). Il piano originale
(componentizzare il rtop wasip1 con WASI) era irrealizzabile: niente
WASI-on-component nel kernel, e wasm-tools rifiuta import core non-WIT.

## Verifica

- `make run-test` PASS (smoke include `rtop --once`: "rtop: uptime=…" via
  write-tty, path componente confermato da `bin/rtop.cwasm` nella ps table).
- `make run-rtop-test`/`tests/rtop-ssh-test.sh` PASS 3/3 consecutivi
  (alt-screen enter/leave, ≥3 frame auto-refresh a ~1 Hz su core AP via SSH,
  'q' quit pulito, prompt ripristinato).
- `cargo test -p rtop` (host): 4/4 parser ok.

## File toccati

- wit/ruos-tui.wit
- tools/wt-tui/src/lib.rs
- user/rtop/{Cargo.toml, src/lib.rs (nuovo), src/sys.rs}; rimossi src/main.rs, src/ansi_backend.rs, src/raw.rs
- kernel/src/wasm/wt/component.rs
- kernel/src/wasm/wt/mod.rs (dispatch component in run_cwasm)
- kernel/src/wasm/host/sysinfo.rs (blob builder condivisi)
- Makefile (regole build/tui.cwasm, build/rtop.cwasm)
- tests/rtop-ssh-test.sh
- docs/api/wit.md (sezione ruos:tui)
