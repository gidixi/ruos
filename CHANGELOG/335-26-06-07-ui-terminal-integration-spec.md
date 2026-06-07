# 335 — Spec: terminale reale integrato nel desktop egui (UI ↔ shell su PTY)

**Data:** 2026-06-07

## Cosa

Importata nel progetto la spec di design per sostituire lo stub `Terminal` del
desktop egui con un **terminale vero**: finestra egui il cui contenuto è una shell
viva (`/bin/shell.wasm`) attaccata a una coppia PTY del kernel, con rendering
VT/ANSI completo (parser `vte` → grid → glyph atlas TTF → dirty-cell blit →
texture egui) e input bidirezionale (tasti → byte/CSI → PTY).

Punti chiave della spec:
- Nuova host fn kernel `term.{open,read,write,resize,close}` (`kernel/src/wasm/wt/term.rs`)
  che wrappa l'API PTY esistente + `spawn_shell_on_pty` (stesso bridge di SSH).
- Estensione del trait `Platform` (gui-core) con i 5 metodi `term_*` + sotto-trait
  `TermIo` + `DeskApp::pump` — rispettando la "regola d'oro" (gui-core resta
  ruos-agnostico, niente OS/host fn dirette).
- Due backend: `ruos-window` (host fn) e `pc-backend` (`portable-pty`, per lo
  sviluppo su PC con shell host vera).
- Unico tassello kernel nuovo oltre alle host fn: `winsize` (ws_col/ws_row) in pty.
- 5 milestone incrementali (vendor vte+grid → atlas → Platform+pc-backend →
  host fn kernel+ruos → wire DeskApp).

## Perché

Allinea il desktop alla north star (GUI egui usabile): un terminale funzionante è
il tassello che rende la shell accessibile dalla GUI, riusando il bridge PTY già
collaudato da SSH. La spec attraversa kernel + submodule `ruos-desktop`.

## File toccati

- docs/superpowers/specs/2026-06-07-ui-terminal-integration-design.md (nuovo)
