# 334 — Launcher dinamico + app Notepad portati su main (app sul boot)

**Data:** 2026-06-07

## Cosa

Integrato in `main` dal branch `livecd` il **meccanismo elegante di caricamento
dinamico delle app** e la nuova app **Notepad**, MA mantenendo tutte le app come
**moduli di boot della ISO** (NON è stato portato il #308 "app fuori dal boot →
solo `/mnt/apps`").

Dal merge di `livecd` (commit `a903c01` + bump submodule `ruos-desktop`
2435b2a → 7cfc270):

- **Launcher dinamico (kernel `wasm/wt/wm.rs`)**: il compositor non ha più una
  lista di app hard-coded. `scan_apps` scandisce le app directory (`APP_DIRS =
  ["/bin", "/mnt/apps"]`) per i `*.cwasm` e chiede a ciascuna di descriversi: una
  app appare nel launcher **se e solo se** esporta `manifest() -> i64` (vedi
  `ruos-window::declare_manifest!`). Aggiunti: `read_app_bytes` (cerca `/bin` poi
  `/mnt/apps`), `extract_manifest` (instantiate usa-e-getta + lettura del record
  `id␟title`), `APP_CATALOG`/`MANIFEST_CACHE` con scan throttellato a ~1 Hz, e la
  host fn **`wm.app_list`** che la shell legge ad ogni frame per costruire il menu.
  `module_by_name` ora cerca in `/bin` poi `/mnt/apps`.
- **Submodule `ruos-desktop` → 7cfc270**: app self-describing (`declare_manifest!`
  in `ruos-window`), catalogo dinamico, e nuova app `notepad` (`apps/notepad-app`
  + `gui-core::desktop::apps::notepad::Notepad`, registrata in `default_apps()`).
- **Plugin `new-app.md`**: doc aggiornata al flusso self-describing.

Differenze rispetto a `livecd` (scelta: **app sul boot**):

- `limine.conf`: le 5 app desktop (`about`, `files`, `terminal`, `system`,
  **`notepad`**) restano **moduli di boot** (sulla ISO avviabile, disponibili
  anche diskless). Su `livecd` erano state rimosse.
- `Makefile`: `iso:`/`test-boot:` copiano di nuovo le 5 app in `$(ISO_ROOT)/bin`
  (aggiunta la copia di `notepad.cwasm`); aggiunta la regola `build/notepad.cwasm`
  e i prerequisiti. NON portati il target `apps-on-disk` né `APP_CWASMS`; `run:`
  torna a `run: iso $(DISK_IMG)` (non rigenera il disco ad ogni avvio). Il drop
  folder `/mnt/apps` resta comunque scansionato a runtime se un disco è montato.

## Perché

Su `main` (mainline avviabile) le app devono stare sulla ISO ed essere disponibili
anche senza disco. Il meccanismo dinamico (manifest scan) funziona identico con le
app in `/bin`: si ottiene il launcher self-describing + Notepad senza la
regressione live-CD del #308 (che resta isolata su `livecd` in attesa del driver
ISO9660/ATAPI).

## File toccati

- limine.conf (riaggiunti i 5 moduli app, incl. notepad)
- Makefile (regola notepad.cwasm; copie app in /bin ripristinate + notepad;
  rimossi apps-on-disk/APP_CWASMS; run torna a iso $(DISK_IMG))
- kernel/src/wasm/wt/wm.rs (launcher dinamico: scan_apps, manifest, wm.app_list)
- ruos-desktop (submodule 2435b2a → 7cfc270: notepad + app self-describing)
- tools/ruos-plugins/ruos-desktop/commands/new-app.md
