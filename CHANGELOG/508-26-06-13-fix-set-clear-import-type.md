# 508 — Fix: wm.set_clear import-type mismatch (notify non istanziava)

**Data:** 2026-06-13

## Cosa
Regressione dal 507 (riportata su VBox: "notify si chiude"). La host fn `wm.set_clear`
era registrata `|caller, rgba: i32| -> i32` (tipo `(i32) -> i32`), ma l'import wasm in
`ruos-window` è `fn set_clear(rgba: i32)` (`(i32) -> ()`). Wasmtime rifiuta
l'instantiate: `incompatible import type for wm::set_clear`. Solo **notify** importa
set_clear (via `new_overlay → wm::set_clear(0)`) → solo notify falliva l'instantiate e
spariva (shell/altre app non lo importano → ok).

Fix: la host fn `wm.set_clear` ora ritorna **nothing** (come `wm.set_overlay`),
combaciando con la decl dell'import. docs/api/wm.md aggiornata.

Verificato (boot headless): `spawn app='notify' win_id=1 live=2` + `mesh render win=1`
(notify renderizza via mesh) + `raster cores=4`, zero panic/instantiate-fail.

## Perché
func_wrap deve combaciare ESATTAMENTE (param + result) con il tipo dell'import wasm,
altrimenti l'instantiate fallisce. set_clear è un setter senza errore → niente return.

## File toccati
- kernel/src/wasm/wt/wm.rs (host fn wm.set_clear: -> () invece di -> i32)
- docs/api/wm.md
