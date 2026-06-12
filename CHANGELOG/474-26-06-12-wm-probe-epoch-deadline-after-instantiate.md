# 474 — wm: probe manifest, deadline epoch DOPO l'instantiate + log sui fallimenti muti

**Data:** 2026-06-12

## Cosa

`kernel/src/wasm/wt/wm.rs`, `extract_manifest` (probe del launcher):

1. **Deadline epoch spostata DOPO l'instantiate.** Prima `set_epoch_deadline(
   PROBE_DEADLINE_TICKS=100)` era armata PRIMA di `linker.instantiate`.
   L'instantiate è lavoro host (mappa l'immagine AOT + mprotect W^X) e per questi
   cdylib wasip1 non esegue codice guest (niente `start` section), ma per un blob
   grosso (viewer rustls ~77 MB) sotto TCG dura >1 s di wall-clock → brucia >100
   tick epoch → la chiamata banale `manifest()` subito dopo trovava la deadline
   GIÀ scaduta → trap immediato → `None` muto → app fuori dal launcher senza alcun
   log. Ora la deadline è armata subito prima di `f.call(manifest)`: copre solo
   l'esecuzione guest (banale, dati const), non il setup host.
2. **Log sui due punti prima muti** (`.ok()?`): `instantiate failed` e
   `manifest() trap` ora emettono `bwarn!("wm", ...)` con il nome dello stem.
   L'assenza dell'export `manifest` resta silenziosa (caso normale "non è
   un'app launcher"). `extract_manifest` prende un nuovo parametro `name: &str`;
   il call-site in `scan_apps` passa lo `stem`.

## Perché

Robustezza + visibilità emerse mentre si inseguiva la sparizione del viewer dal
launcher. NB: la causa ROOT della sparizione NON era qui (il viewer non arrivava
nemmeno al probe — veniva scartato a monte in `unpack_bin`, vedi changelog 475);
ma queste due correzioni restano valide a sé:
- il changelog 450 aveva aggiunto il log solo al `deserialize`
  (`module_at_path`), mentre instantiate + `manifest()` di `extract_manifest`
  erano interamente `.ok()?` → un probe fallito spariva senza traccia;
- un blob grosso (77 MB) AVREBBE comunque false-trappato il probe per via
  dell'instantiate lento sotto TCG che bruciava il budget epoch (455) prima
  della chiamata `manifest()`. Spostare la deadline dopo l'instantiate la rende
  semanticamente corretta (watchdog dell'esecuzione guest, non del setup host).

## File toccati
- kernel/src/wasm/wt/wm.rs
