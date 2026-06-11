# 450 — wm: log su fallimenti silenziosi + viewer.cwasm ri-precompilato (tunables 422)

**Data:** 2026-06-11

## Cosa

1. `kernel/src/wasm/wt/wm.rs`: i punti di fallimento che inghiottivano l'errore
   ora loggano `bwarn`: deserialize in `module_for` / `module_by_name` /
   `module_at_path` (probe manifest del launcher), trap in `_initialize`
   (`run_initialize`) e trap/exit di `frame()` in `frame_all`.
2. Ri-precompilati con il `tools/wt-precompile` corrente (post-changelog 422):
   `apps/viewer.cwasm`, blob embedded `kernel/src/wasm/wt/viewer.cwasm` e
   `viewer-gate.cwasm` (+ `testapp.cwasm` nel workspace SDK `ruos-test`).
   Sorgenti `.wasm` invariati — solo lo step AOT rifatto.

## Perché

App esterna `apps/viewer.cwasm` sparita dal launcher senza alcun log. Causa:
changelog 422 (2026-06-10) ha cambiato `memory_reservation` 0 → 256 MiB in
kernel e wt-precompile; i `.cwasm` precompilati prima sono rifiutati al
deserialize ("Module was compiled with a memory reservation of '0' but
'268435456' is expected") ma l'errore era silenziato (`.ok()?`) → la probe del
manifest li trattava come "senza manifest" e i boot-check blitz tornavano
pixels=0 senza spiegazione. Le app di sistema non erano affette perché
ricompilate ad ogni `make iso`. Regola operativa: dopo un cambio di tunables
Wasmtime, ogni `.cwasm` esterno (drop folder `apps/`, `/mnt/apps`) va
ri-precompilato; ora il kernel lo segnala esplicitamente in seriale.

## File toccati

- kernel/src/wasm/wt/wm.rs
- apps/viewer.cwasm (rigenerato)
- kernel/src/wasm/wt/viewer.cwasm (rigenerato)
- kernel/src/wasm/wt/viewer-gate.cwasm (rigenerato)
