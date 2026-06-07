# 300 — egui compositor SP-E: DeskApps as windows, gui.cwasm retired (Model A complete)

**Data:** 2026-06-05

## Cosa

SP-E è fatto: il **desktop Model-A è COMPLETO**. Le quattro `gui-core` DeskApp
(`AboutRuos` / `Files` / `Terminal` / `System`) sono ora app-finestra reali del
compositor, ciascuna incapsulata in un thin crate wasip1 nel submodule
`ruos-desktop`:

- `about-app`, `files-app`, `terminal-app`, `system-app` — ognuno un `cdylib`
  reactor (`#[no_mangle] frame`/`_start`) sopra la SDK `ruos-window`. `frame()`
  inizializza `static mut S: WindowState` + `static mut APP: <DeskApp>` (pattern
  single-thread: `frame` è l'unico accessor e non è mai rientrante, il kernel lo
  chiama serialmente una finestra per volta), poi:
  `ruos_window::frame_once(s, app.title(), W, H, |ctx| CentralPanel{ app.ui(ui) })`.
  Costruzione DeskApp: `AboutRuos`/`Files` unit struct, `Terminal::default()`,
  `System::default()`. Surface: 560×420 per about/files/terminal, 720×520 per
  system (tabella + grafici).
- Build: `cargo build -p <crate> --target wasm32-wasip1 --release` →
  `<id>_app.wasm` → AOT-precompilato in `build/<id>.cwasm` → shippato a
  `/bin/<id>.cwasm` (sia in `iso:` che in `test-boot:`) → montato via `limine.conf`
  (4 coppie `module_path`/`module_cmdline`, sullo stesso pattern della shell).
- Il launcher della shell SP-D ("☰ Apps", voci `about`/`files`/`terminal`/`system`)
  ora apre **finestre app reali** via `wm.spawn(id)` → il kernel carica
  `/bin/<id>.cwasm`. (id == filename, verificato in `wm.rs`.)

**`gui.cwasm` RITIRATO** dall'ISO di default e dal comando `gui`:
- rimosso da prereq + `cp` di `iso:` e `test-boot:`, e dalla entry `limine.conf`;
- il codice resta in `ruos-backend`/`gui-core` `Desktop` ed è ancora ricostruibile
  con `make build/gui.cwasm` (la regola `build/gui.cwasm:` è KEPT) — solo
  non-shippato, nessuna modifica al kernel; `gui` si risolve via il path generico
  `/bin`.

Verificato QEMU+KVM (`build/spe_verify.py`): ciascuna di About ("About ruOS") /
Files / Terminal / Activity Monitor si apre come finestra CSD (titlebar +
contenuto; Activity Monitor = tab + tabella processi + grafici CPU); multi-finestra
(About + Files in cascata); `wm.spawn ok name=...` per ogni app nel seriale;
`gui.cwasm` assente da `/bin`; `ruos-backend` continua a buildare; desktop
renderizzato anche su VirtualBox.

## Perché

Chiudere SP-E e completare il desktop **Model A**: ogni app del desktop è una
finestra WASM separata spawnata dal launcher (non più il monolite `gui.cwasm` che
disegnava tutte le "app" dentro un'unica finestra egui). Riusa il rendering
`gui-core` invariato; ritirare `gui.cwasm` dall'ISO evita di shippare due percorsi
GUI sovrapposti.

## Note per SP-F

- **(a) Budget heap ~5 finestre egui.** shell + 4 app ≈ 5×48 MiB ≈ 240 di 256 MiB.
  Aprire una 5ª+ fallisce in modo grazioso (`wm.spawn` → 0, niente finestra, niente
  panic). SP-F tara l'heap.
- **(b) VirtualBox richiede ≥1024 MiB RAM.** A 512 MiB della VM, EFI/Limine espone
  solo ~304 MiB usabili → la regione heap contigua da 256 MiB fallisce
  (`HeapInit no usable region`). QEMU `-m 512` è OK.
- **(c) Dati placeholder/simulati.** System: processi hardcoded; Terminal: echo
  stub. I dati reali (`proc::list`/CPU/mem, terminale PTY reale) sono SP-F.
- **(d)** Il titolo finestra della system app è "Activity Monitor" (launcher id
  `system`).

Riferimenti: spec `docs/superpowers/specs/2026-06-05-egui-compositor-sp-e-*` +
piano SP-E, e `[[vbox-test-harness]]`.

## File toccati

- ruos-desktop/about-app/{src/lib.rs,Cargo.toml}
- ruos-desktop/files-app/{src/lib.rs,Cargo.toml}
- ruos-desktop/terminal-app/{src/lib.rs,Cargo.toml}
- ruos-desktop/system-app/{src/lib.rs,Cargo.toml}
- ruos-desktop/Cargo.toml (workspace members)
- Makefile (4 regole build/<id>.cwasm; ship in iso:/test-boot:; gui.cwasm tolto dai prereq/cp, regola build/gui.cwasm: KEPT)
- limine.conf (4 coppie module app; entry gui.cwasm rimossa)
- build/spe_verify.py
