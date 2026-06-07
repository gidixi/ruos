# 307 — Launcher dinamico: app da cartella, non da codice

**Data:** 2026-06-07

## Cosa

Il launcher del desktop non ha più una lista di app cablata nel codice
(`CATALOG` statico nello `shell` + entry hard-coded). Ora il compositor
**scopre le app a runtime** scandendo `/bin` e `/mnt/apps` per `*.cwasm`:
un'app compare nel launcher **se e solo se** esporta `manifest()` (auto-descrizione,
stile bundle). Conseguenza pratica: per aggiungere un'app basta buildare il suo
`.cwasm` e **droppare il file in `/mnt/apps`** (FAT32 persistente) — niente
rebuild, niente `limine.conf`, niente `Makefile`, niente edit di liste. Appare
entro ~1 s (scan throttlato a 1 Hz). Le core app già spedite in `/bin` come moduli
Limine vengono scoperte automaticamente allo stesso modo.

Dettagli ABI:
- **`manifest() -> i64`** (nuovo export opzionale): ritorna `(ptr<<32)|len` di un
  record UTF-8 `id\u{1f}title\u{1f}w\u{1f}h` nel data segment del guest (const, no
  heap → leggibile dal kernel subito dopo l'instanziazione, senza `_initialize`).
  Emesso dalla macro `ruos_window::declare_manifest!(id, title, w, h)`.
- **`wm.app_list(ptr, max) -> count`** (nuova host fn): scrive il catalogo corrente
  (record da 64 B: id 24 B + title 40 B, UTF-8 NUL-padded). Lo `shell` la chiama
  ogni frame per costruire il menu.
- `wm.spawn` / `module_by_name` ora cercano `<name>.cwasm` in `/bin` **poi**
  `/mnt/apps` (le app droppate diventano spawnabili).
- Il compositor mantiene un memo per-stem dei manifest e ricostruisce il catalogo
  dai file attualmente presenti (cancellare un `.cwasm` lo toglie dal launcher).

`shell`/`compositor` sono infrastruttura: non esportano `manifest()` (ed sono in
una EXCLUDE list per evitare l'instanziazione sprecata) → assenti dal launcher.

## Perché

Il vecchio meccanismo richiedeva ~8 punti di edit a build-time per ogni app
(DeskApp, registrazione gui-core, crate, workspace, Makefile, `limine.conf`,
`CATALOG`, copia ISO) e bakeava tutto nell'immagine di boot. Allinea il progetto
alla north star ("eseguire app `.wasm`"): le app vivono in una cartella e il
kernel le pesca da lì (modello `/bin` di Linux / System di Windows), invece di
essere cablate. La registrazione `gui-core::default_apps()` resta solo per
l'anteprima PC (`pc-backend`), non più necessaria on-device.

## File toccati

- kernel/src/wasm/wt/wm.rs (scan manifest, `APP_CATALOG`, `wm.app_list`,
  `module_by_name` multi-dir, hook nel run loop)
- ruos-desktop/crates/ruos-window/src/lib.rs (`declare_manifest!`, `app_list()`,
  extern `wm.app_list`)
- ruos-desktop/crates/gui-core/src/desktop/shell.rs (`ShellAppEntry`/`ShellIntents`
  owned `String`, fallback "No apps found")
- ruos-desktop/apps/shell/src/lib.rs (catalogo dinamico via `app_list()`,
  rimosso `CATALOG` statico)
- ruos-desktop/apps/{about,files,terminal,system,notepad}-app/src/lib.rs
  (`declare_manifest!`)
