# 306 — Doc "implementare un'app" + plugin Claude `/new-app`

**Data:** 2026-06-06

## Cosa

Strumenti per creare nuove app del desktop.

- **`ruos-desktop/docs/implementing-an-app.md`** (nuova): come **scrivere** la `ui()`
  di un'app egui — immediate-mode, stato nella struct, layout/contenitori, widget,
  tab, tabelle `egui_extras`, disegno custom col `painter`, input/tempo/animazione
  (con la nota perf su `request_repaint()` legata al commit-on-damage) e i vincoli
  del raster software. Fondata sui pattern reali (Terminal/Files/About/System).
  Complementare a `adding-an-app.md` (che copre il **cablaggio**). README +
  adding-an-app si linkano alla nuova guida.

- **Plugin Claude Code `ruos-desktop`** (`tools/ruos-plugins/`): marketplace locale
  `ruos-local` + plugin `ruos-desktop` con il comando **`/ruos-desktop:new-app`** che
  fa lo scaffold end-to-end di una nuova app (DeskApp + crate cdylib + workspace
  member + regola Makefile + voce CATALOG) e builda. La ricetta col template è dentro
  il comando → invocazioni deterministiche.

## Perché

Abbassare l'attrito per aggiungere app: una guida su *come si scrive* la UI (non solo
dove vanno i file) e un plugin che genera lo scheletro dei 5 punti in un colpo.

## Note

- Il package del plugin è tracciato (`tools/`), condivisibile col repo; lo stato di
  *installazione* vive nei settings Claude locali (`.claude`, gitignored).
- Install: `/plugin marketplace add ./tools/ruos-plugins` →
  `/plugin install ruos-desktop@ruos-local`. Uso: `/ruos-desktop:new-app <id> [Titolo] [WxH]`.

## File toccati

- ruos-desktop/docs/implementing-an-app.md (nuova)
- ruos-desktop/docs/adding-an-app.md (cross-link), ruos-desktop/README.md (pointer)
- tools/ruos-plugins/.claude-plugin/marketplace.json
- tools/ruos-plugins/ruos-desktop/.claude-plugin/plugin.json
- tools/ruos-plugins/ruos-desktop/commands/new-app.md
