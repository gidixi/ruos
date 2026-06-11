# 451 — docs: regola compatibilità `.cwasm` esterni vs tunables Wasmtime

**Data:** 2026-06-11

## Cosa

Documentata la regola emersa col changelog 450: sezione "`.cwasm`
compatibility" in `docs/api/README.md` (il manuale che l'SDK copia in ogni
progetto) e avviso in `apps/README.md` (il drop folder), con sintomo (riga
WARN in seriale), causa (AOT legato ai tunables dell'engine) e procedura di
ri-precompilazione (build.ps1 SDK oppure wt-precompile diretto).

## Perché

Un `.cwasm` esterno che smette di caricare dopo un cambio di tunables è un
fallimento non ovvio (changelog 422 → viewer sparito dal launcher senza log).
La regola va dove la incontra chi sviluppa app: il README del drop folder e il
manuale API distribuito dall'SDK.

## File toccati

- docs/api/README.md
- apps/README.md
