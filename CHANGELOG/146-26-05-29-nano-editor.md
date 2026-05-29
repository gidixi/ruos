# 146 — nano-style text editor wasm tool

**Data:** 2026-05-29

## Cosa

`user/nano/`: editor di testo minimal stile GNU nano (~330 LoC).

- Apre file via `argv[1]`; se assente, parte da buffer vuoto +
  creazione on-save
- Terminale 80×24 hard-coded (24 righe contenuto + status + help)
- Raw line discipline via `tcgetattr`/`tcsetattr` (stesso pattern
  shell)
- Buffer = `Vec<String>` una entry per riga
- Cursor (line, col) + scroll-on-leave-viewport
- Keystroke:
  - ASCII stampabile → insert at cursor
  - `Enter`           → split line
  - `Backspace`       → del prev (join con riga sopra a col 0)
  - Arrows ESC[A/B/C/D → muovi cursor
  - `Home`/`End` ESC[H/F → col 0 / fine riga
  - `^O` (Ctrl-O)     → save
  - `^X` (Ctrl-X)     → exit
- Status bar invertita: nome file + flag modified `*` + Ln/Col
- Help footer invertito: `^O Save  ^X Exit  Arrows / Home / End move`

Use case: editare init.sh o file qualsiasi su `/mnt` (FAT persistent)
o `/tmp` (RAM tmpfs).

## Build

Aggiunto a `user/Cargo.toml` workspace, `Makefile BIN_TOOLS`,
`limine.conf`.

## Test

`make run-test` → TEST_PASS (nessuna regressione). Test interattivo:
```
ruos:/$ nano /mnt/init.bak
[edita, ^O salva, ^X esci]
```

## Limitazioni

- Solo ASCII (no UTF-8 multi-byte insertion testata)
- No syntax highlighting
- No search (^W)
- No undo
- No horizontal scroll (riga > 80 char troncata in display)
- Terminale dimensione hardcoded — niente TIOCGWINSZ in ruos shell

## File toccati

- user/nano/Cargo.toml (nuovo)
- user/nano/src/main.rs (nuovo, ~330 righe)
- user/Cargo.toml (workspace member `nano`)
- Makefile (BIN_TOOLS += nano)
- limine.conf (module /bin/nano.wasm)
- CHANGELOG/146-26-05-29-nano-editor.md (questo)
