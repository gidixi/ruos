# 302 — Riorganizzazione workspace ruos-desktop (crates/ apps/ backends/)

**Data:** 2026-06-06

## Cosa

Riordino del submodule `ruos-desktop`, allineandolo al Model A (ogni finestra è
un'app `.cwasm` separata) e rimuovendo il debito del vecchio monolite.

**Submodule `ruos-desktop` (branch `reorg-workspace`):**

- Rimossi due crate morti:
  - `ruos-backend` — produceva `gui.cwasm`, ritirato dall'ISO al pivot Model A
    (la sua regola Make era già orfana, vedi sotto).
  - `wt-harness` — diagnostica del `gui.wasm` monolitico ritirato; era un
    workspace annidato a sé (Cargo.lock proprio).
- Crate raggruppate per ruolo (da layout piatto a tre gruppi):
  - `crates/` → `gui-core`, `ruos-window` (librerie portabili, copiate in ruos).
  - `apps/` → `shell`, `about-app`, `files-app`, `terminal-app`, `system-app`,
    `compositor-app` (cdylib wasm, un `.cwasm` ciascuna).
  - `backends/` → `pc-backend` (anteprima PC, throwaway, solo Windows).
- `Cargo.toml` del workspace: `members`/`default-members` aggiornati ai nuovi
  path, `ruos-backend` rimosso; path-dep interne risistemate ai nuovi livelli
  (`../../crates/...`).
- `README.md` + `CLAUDE.md` riscritti: erano fermi a un modello a 4 crate e al
  flusso `gui.cwasm`; ora descrivono il Model A, le 8 crate vive, cosa spedisce a
  ruos vs throwaway, e l'accoppiamento col Makefile del padre.

**Repo `ruos` (branch `reorg-ruos-desktop`):**

- `Makefile`: prereq `find`/`wildcard` delle regole `.cwasm` aggiornati ai nuovi
  path del submodule (`ruos-desktop/crates/...`, `ruos-desktop/apps/...`).
- `Makefile`: rimossa la regola morta `build/gui.cwasm` + la variabile
  `RUOS_DESKTOP_SRCS` (orfane dal ritiro del monolite; non più prerequisito di
  `iso:`/`test-boot:`).

## Perché

Il workspace era cresciuto a 10 crate con layout piatto e doc fermi a 2-3
architetture prima: impossibile distinguere a colpo d'occhio cosa gira on-device
(le app Model A) da cosa è anteprima PC throwaway, e due crate morti restavano in
albero. Il raggruppamento `crates/ apps/ backends/` rende esplicito il confine
"spedito a ruos vs PC", e la rimozione dei morti + la regola Make orfana toglie
rumore e trappole (rebuild di output mai più montati).

## Verifica

- WSL: `cargo build --target wasm32-wasip1 --release` di tutte le 6 crate app →
  OK; `cargo test -p gui-core` → 10/10 verde.
- `make build/{about,files,terminal,system}.cwasm`,
  `kernel/src/wasm/wt/{shell,egui_demo}.cwasm` → tutti costruiti (MAKE_EXIT=0),
  a conferma che i path aggiornati del Makefile risolvono end-to-end.
- `pc-backend` non verificabile qui (richiede cargo host Windows, assente nel
  sandbox); manifest aggiornato e invariato nella logica.

## File toccati

- ruos-desktop/Cargo.toml
- ruos-desktop/README.md
- ruos-desktop/CLAUDE.md
- ruos-desktop/{crates,apps,backends}/** (spostamento via `git mv`; path-dep nei Cargo.toml)
- ruos-desktop/ruos-backend/** (rimosso)
- ruos-desktop/wt-harness/** (rimosso)
- Makefile (path submodule aggiornati; regola gui.cwasm rimossa)
