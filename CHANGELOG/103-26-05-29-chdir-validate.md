# 103 — chdir validates target exists + is Dir

**Data:** 2026-05-29

## Cosa

Bug visibile: da `/bin`, `cd bin` produceva `/bin/bin` (kernel CWD
appende silenziosamente) anche se `/bin/bin` non esiste. Conseguenza:
`ls` poi falliva con ENOENT 44 (giusto), ma user-experience confusa.

Fix `ruos::chdir`:
1. Resolve target via `resolve_cwd(&caller.data().cwd, path)`.
2. Se resolved != `/`, stat via `vfs::block_on(vfs::stat(&path))`.
3. Se kind == Dir → update cwd, ritorna 0.
4. Se kind != Dir → ritorna 54 (ENOTDIR).
5. Se stat fail → ritorna 44 (ENOENT).

shell.wasm `builtin_cd` già early-return su errno != 0 e printa
`cd: <target>: errno N`. User vede chiaro feedback.

Root path skipped (sempre esiste; evita corner case).

## Perché

Validation lato kernel = single source of truth. Shell-side local
CWD mirror resta in sync.

## File toccati

- kernel/src/wasm/host/proc.rs
- CHANGELOG/103-26-05-29-chdir-validate.md (nuovo)
