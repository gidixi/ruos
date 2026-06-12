# 477 — Interprete Lua in-OS (lua.wasm)

**Data:** 2026-06-12

## Cosa

Scaffolding per portare l'interprete Lua come tool `.wasm` in `/bin`, così ruos
può eseguire script `.lua` in-OS (`lua /home/script.lua`). Lua è C, non un crate
cargo: vive fuori dal pipeline `user/*` → `user-bin/*.wasm`.

- `third_party/lua/build-wasm.sh` — script ripetibile (WSL) che scarica
  `wasi-sdk` + sorgente Lua 5.4.7 e builda lo standalone `lua` come modulo
  wasm32-wasi. Flag: `LUA_USE_C89` (no POSIX/popen/system/readline/dlopen),
  `_WASI_EMULATED_SIGNAL` + `-lwasi-emulated-signal` (handler ^C). Output:
  `user-bin/lua.wasm`.
- `third_party/lua/README.md` — uso, flag e motivi, limiti noti (no subprocess,
  WASI gaps da scoprire al boot, setjmp/longjmp, doppia interpretazione = lento).
- Makefile: `lua` aggiunto a `BIN_TOOLS` (→ packato in `bin.bgz` → `/bin`) +
  regola esplicita `user-bin/lua.wasm` che ombreggia la pattern-rule cargo:
  tratta il wasm come artefatto prebuilt (no rebuild) e, se mancante, rimanda
  allo script invece di fallire con errore cargo criptico.
- `.gitignore`: eccezione `!user-bin/lua.wasm` — il prebuilt va committato come
  artefatto vendored (no sorgente in-repo).

## Perché

Primo passo del path "interprete in-OS": scrivi codice dentro ruos (editor
`nano` già presente) ed eseguilo senza compilare a nativo. Zero modifiche al
kernel — `lua.wasm` gira sul runtime `wasmi` esistente come ogni altro tool WASI.
Build effettiva di `lua.wasm` + commit dell'artefatto pendenti (richiedono WSL +
download rete, da eseguire a mano).

## File toccati

- third_party/lua/build-wasm.sh
- third_party/lua/README.md
- Makefile
- .gitignore
- CHANGELOG/477-26-06-12-lua-wasm-interpreter.md
