# 102 — Tab path completion for file/folder names

**Data:** 2026-05-29

## Cosa

User bug: tab dopo nome comando NON completava file/folder. Solo
builtin + `/bin/*.wasm` venivano proposti, anche per `cat foo` o
`ls /etc`.

Fix: `tab_complete` ora distingue first-token (command) vs subsequent
(path):

```rust
fn tab_complete(first_token: bool, prefix: &[u8]) -> Vec<String> {
    if first_token { complete_command(prefix) }
    else           { complete_path(prefix) }
}
```

`complete_path`:
1. Split prefix at last `/` → `(dir, name_prefix)`. Nessuno slash =
   `(".", prefix)`.
2. `readdir(dir)` (resolves via kernel CWD se relative).
3. Filter entries `.starts_with(name_prefix)`.
4. Ritorna candidati preservando `dir/` + nome + `/` trailing su Dir.

Es:
- `cat /et<TAB>` → dir=`/`, prefix=`et`, readdir / → trova `etc`
  Dir → candidato `/etc/` → cursor avanza.
- `ls bi<TAB>` (in /) → dir="", prefix=`bi`, readdir "." → trova `bin`
  Dir → candidato `bin/`.

`readdir_entries(path)` ora ritorna `Vec<(name, is_dir)>` cosi
trailing `/` distingue dir da file.

## Perché

Tab completion era utile solo per i comandi. Path completion = QoL
shell standard.

## File toccati

- user/shell/src/main.rs (+complete_command +complete_path +readdir_entries
  +dispatcher tab_complete; call site con first_token detection)
- user-bin/shell.wasm (rebuilt)
- CHANGELOG/102-26-05-29-tab-path-completion.md (nuovo)
