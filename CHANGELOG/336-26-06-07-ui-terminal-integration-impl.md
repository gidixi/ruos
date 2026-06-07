# 336 — Terminale reale nel desktop egui: implementazione (UI ↔ shell su PTY)

**Data:** 2026-06-07

## Cosa

Implementata la spec 335: lo stub `Terminal` del desktop egui è ora un
**terminale vero** — una finestra il cui contenuto è una shell viva
(`/bin/shell.wasm`) su una coppia PTY del kernel, con parser VT/ANSI, character
grid, glyph atlas e input bidirezionale.

**Kernel (`ruos`):**
- `kernel/src/wasm/wt/term.rs` (nuovo): host module `term` con
  `open/read/write/resize/close`, wrapper sopra l'API PTY esistente
  (`try_claim`, `master_output_try`, `master_input_push`, `is_claimed`,
  `master_output_len`, `request_shutdown`, `release`) + `spawn_shell_on_pty`.
  Stesso bridge di SSH (`ssh/sunset_io.rs`): handle = indice coppia PTY (parte da
  1, la 0 è la console di boot); `read` non bloccante, `-1` = EOF.
- `kernel/src/pty/mod.rs`: tabella `WINSIZE` (atomics) + `set_winsize`/`winsize`.
  **NON** aggiunta a `Termios` (è `repr(C)` memcpy verso wasi-libc: aggiungere
  campi corromperebbe tcgetattr/tcsetattr).
- `kernel/src/wasm/wt/mod.rs`: `pub mod term;`.
- `kernel/src/wasm/wt/wm.rs`: `term::add_to_linker` registrato ai 4 siti che
  costruiscono il `Linker<AppState>` delle finestre.
- `wit/ruos-gui.wit`: `interface term` + `import term` nel world (doc dell'ABI;
  l'impl concreta è raw, come per `wm.poll_event`).

**gui-core (PORTABILE, `ruos-desktop`):**
- `platform.rs`: `TermHandle` + trait `TermIo` (5 metodi `term_*`, default no-op);
  `Platform: TermIo`.
- `desktop/app_trait.rs`: `DeskApp::pump(&mut dyn TermIo)` (default no-op),
  chiamato dal driver prima di `ui()`.
- `desktop/apps/term/{grid,vt,atlas}.rs` (nuovi): `Grid`/`Cell`/`Attrs`/`Color`
  + scrollback + dirty per-riga; `GridPerform` (`impl vte::Perform`) + porting
  `apply_sgr` (16-color/xterm-256/truecolor); glyph atlas con
  `noto-sans-mono-bitmap` (size 16) + compose tint-on-blit in un pixbuf RGBA.
- `desktop/apps/terminal.rs` (riscritto): `pump()` (open lazy, drena output →
  `vte` → grid → blit celle dirty, invia i tasti accodati, gestisce EOF) +
  `ui()` (upload texture egui del pixbuf, `painter().image`, focus al click,
  cursore, tasti → byte/CSI, resize → cols/rows → `term_resize`).
- Deps: `vte` (no_std, stessa versione del kernel) + `noto-sans-mono-bitmap`
  (entrambe pure-Rust → dentro la regola d'oro).

**Backend ruos:**
- `crates/ruos-window/src/lib.rs`: `mod term` (`#[link(wasm_import_module="term")]`)
  + `RuosTermIo` che implementa `TermIo` sulle host fn.
- `apps/terminal-app/src/lib.rs`: `app.pump(&mut RuosTermIo)` prima di `frame_once`.

**Backend PC (throwaway):**
- `backends/pc-backend`: `impl TermIo for PcPlatform {}` (default no-op) per
  compilare. La shell host su PC (`portable-pty`) + il threading di `pump` nel
  desktop monolitico restano un follow-up del dev-loop; su ruos il terminale è
  completamente cablato.

## Scelte / scostamenti dalla spec

- **Glyph atlas:** usato `noto-sans-mono-bitmap` (fallback (b) della spec) invece
  di `ab_glyph`+TTF — pure Rust, già nel kernel, niente blob font da embeddare.
- **Winsize:** memorizzata fuori da `Termios` (ABI-locked) in una tabella atomica
  dedicata in `pty`.
- **Render dirty:** blit per-cella su span dirty per-riga; lo scroll marca le righe
  dirty e ri-blitta (l'ottimizzazione memmove della spec è un follow-up).
  Upload texture intero quando qualcosa cambia (il `set_partial` per solo-bbox è
  un follow-up); frame statici = nessun upload.

## Verifica

- `cargo build`: gui-core ✓, terminal-app (wasm32-wasip1) ✓, pc-backend (host) ✓,
  kernel ✓.
- `make iso` + `make run-test` (vedi commit).

## File toccati

- kernel/src/wasm/wt/term.rs (nuovo), kernel/src/wasm/wt/mod.rs,
  kernel/src/wasm/wt/wm.rs, kernel/src/pty/mod.rs, wit/ruos-gui.wit
- ruos-desktop: crates/gui-core/src/platform.rs,
  crates/gui-core/src/desktop/app_trait.rs,
  crates/gui-core/src/desktop/apps/mod.rs,
  crates/gui-core/src/desktop/apps/terminal.rs,
  crates/gui-core/src/desktop/apps/term/{mod,grid,vt,atlas}.rs,
  crates/gui-core/Cargo.toml, Cargo.toml,
  crates/ruos-window/src/lib.rs, apps/terminal-app/src/lib.rs,
  backends/pc-backend/src/main.rs
