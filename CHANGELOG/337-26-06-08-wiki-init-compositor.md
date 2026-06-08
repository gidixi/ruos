# 337 — Wiki di progetto: struttura + regole + pagina compositor

**Data:** 2026-06-08

## Cosa

Inizializzata la wiki di progetto in `docs/wiki/` (markdown):

- `README.md` — home/indice della wiki, separazione di ruolo da CHANGELOG e spec.
- `STYLE.md` — regole di scrittura: scopo delle pagine, naming kebab-case,
  intestazione obbligatoria (Stato/Aggiornato/Fonti/Spec), regole di link al
  codice, struttura di una pagina di componente, lingua italiana.
- `architecture/overview.md` — stub di panoramica architetturale (boot a fasi,
  due runtime WASM).
- `components/compositor.md` — prima pagina di componente completa: descrive il
  compositor / window manager kernel-side (Model A, tipi `Compositor`/`Window`/
  `WmState`, loop `run`, input routing, compositing parallelo a bande SMP,
  present-gating, CSD, host fn `wm`, launcher dinamico, vincoli e insidie).

## Perché

Serve una vista stabile e navigabile del sistema ("come è fatto e perché"),
distinta dalla traccia storica (changelog) e dai documenti di design (spec). Si
parte dal compositor; il resto dei sottosistemi è elencato come TODO nell'indice e
si riempirà un componente alla volta seguendo `STYLE.md`.

## File toccati
- docs/wiki/README.md
- docs/wiki/STYLE.md
- docs/wiki/architecture/overview.md
- docs/wiki/components/compositor.md
- CHANGELOG/337-26-06-08-wiki-init-compositor.md
