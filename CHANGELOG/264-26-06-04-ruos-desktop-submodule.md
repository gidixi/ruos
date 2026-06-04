# 264 — ruos-desktop come git submodule

**Data:** 2026-06-04

## Cosa
Agganciato il repo GUI `gidixi/ruos-desktop-ui` come git submodule in
`ruos-desktop/` (contiene gui-core, ruos-backend, pc-backend, wt-harness — il
sistema per creare/compilare la GUI per Windows e per ruos). Repuntato il
Makefile `RUOS_DESKTOP ?= ../../M/ruos-desktop` → `ruos-desktop` così
`make build/gui.cwasm` builda dal submodule in-tree invece che dal sibling
esterno.

## Perché
La GUI (egui desktop) viveva in un repo PC esterno referenziato per path
relativo. Portarla in-tree come submodule rende la build riproducibile e versiona
il riferimento, prerequisito per indagare il bug "testo egui garbled solo su
ruos" (tool diagnostico `ruos-desktop/wt-harness`, CHANGELOG 262).

## File toccati
- .gitmodules (nuovo)
- ruos-desktop (gitlink submodule)
- Makefile
- CHANGELOG/264-26-06-04-ruos-desktop-submodule.md
