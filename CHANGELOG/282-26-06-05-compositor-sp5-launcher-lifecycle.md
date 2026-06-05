# 282 — Compositor SP5: launcher + lifecycle

**Data:** 2026-06-05

## Cosa
Ultimo sotto-progetto del compositor multi-finestra (piano
`2026-06-05-compositor-sp5-launcher-lifecycle.md`, contract decision 8): lanciare
app come nuovi processi wasm a runtime da un launcher, e teardown pulito.

- **Registry app** `APPS: &[AppEntry{name, cwasm}]` (react-A, react-B → stesso
  `reactor.cwasm`; selfclose → nuovo `reactor_close.cwasm`) + **cache `Module`**
  (`MODULE_CACHE`, key = ptr del blob, deserialize una volta, `Module::clone`
  Arc-cheap → istanze multiple a basso costo).
- **`spawn_app(idx)`**: budget guard (`MAX_WINDOWS=8`), alloca un window-id
  (free-list + high-water), istanzia un fresco `(Store<WmState>, Instance)`,
  `proc::register("win:<name>")`, piazza la finestra a cascata (rispetta
  `sy >= TITLE_H`), `raise`+`set_focus`. Ritorna l'id (None se budget pieno /
  modulo non valido — id liberato in caso di fallimento instantiate).
- **Teardown unificato**: `remove_at(i)` = unica via d'uscita (drop Store/Instance
  → libera la linear memory guest + il surface buffer, `proc::unregister(pid)`,
  ricicla l'id nella free-list, fixa `focused`). `close(id)` (dal [X] di SP3) e
  `reap()` passano entrambi da lì. `reap()` (primo statement del loop) promuove
  `close_requested` (settato dall'host fn `wm.close()`) → `alive=false` e rimuove
  le finestre morte.
- **Launcher taskbar**: striscia in basso (`LAUNCHER_H=28`, bottoni
  `LAUNCHER_BTN_W=96`) disegnata dal kernel (`draw_launcher`, etichette via
  `decor::draw_text`), `launcher_hit` mappa il click all'indice app; in
  `on_left_down` un click sulla taskbar fa `spawn_app` e consuma il click (return
  prima del dispatch finestra). `draw_launcher` chiamato dopo `present()` ogni
  frame (sempre on-top).
- Guest `tools/wt-reactor-close` (no_std): importa `wm.close`, si auto-chiude al
  3° frame (prova il path guest→compositor close + teardown).

## Verifiche
- Boot-check headless: `launcher registry apps=3 modules_ok=3`;
  `lifecycle spawns=1 peak_live=1 final_live=0` (selfclose spawnato → close al
  frame 3 → reaped); `reaped win_id=0 (Store/Instance dropped)`;
  `lifecycle reuse: new win_id after recycle = 0` (id riciclato dalla free-list).
- Visual QEMU+KVM+QMP: taskbar con 3 bottoni; click react-A → 3ª finestra
  (`spawn app='react-A' win_id=2 live=3`); selfclose si auto-reap; click [X]
  della finestra spawnata → `reaped win_id=2`. Screendump confermano.
- Review finale (spawn/reap/free-list/focus-fixup/id-leak/deadlock/bounds):
  **pulita**.

## Perché
Completa il compositor multi-finestra: spec §4 item 5. Le app diventano processi
spawnabili/chiudibili a runtime dalla UI — base per app reali (es. il system-info)
e per un futuro registry on-disk (`/bin`-scanned).

## File toccati
- tools/wt-reactor-close/{Cargo.toml, src/lib.rs} (nuovo)
- kernel/src/wasm/wt/wm.rs
- kernel/src/wasm/wt/mod.rs
- kernel/src/boot/phases/interrupts.rs
- Makefile (build + ship reactor_close.cwasm)
- build/launch_verify.py (nuovo, driver QMP)
