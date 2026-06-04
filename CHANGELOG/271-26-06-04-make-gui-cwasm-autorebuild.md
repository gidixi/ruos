# 271 — Build: gui.cwasm si ricompila quando cambiano i sorgenti del submodule

**Data:** 2026-06-04

## Cosa
La regola `build/gui.cwasm` ora dipende dai sorgenti Rust di
`ruos-desktop/gui-core` + `ruos-desktop/ruos-backend` (e dai loro manifest), così
`make iso` ricompila automaticamente il `.cwasm` quando modifichi la GUI.

## Perché
Prima la regola dipendeva SOLO dal tool precompilatore (`$(WT_PRECOMPILE)`), non
dai sorgenti. Quindi dopo aver editato il submodule, `make` vedeva `gui.cwasm`
"up to date" e **saltava** la ricompilazione → la build usava un `.cwasm` vecchio.
Bisognava forzare con `rm -f build/gui.cwasm`. (Il kernel non aveva il problema:
`build` è `.PHONY` → cargo gira sempre, incrementale.)

## Come
`RUOS_DESKTOP_SRCS` enumera i `.rs` via `find` (a parse-time) + i `Cargo.toml`/
`Cargo.lock` via `wildcard`, aggiunti come prerequisiti di `build/gui.cwasm`. Se
il submodule non è checked-out la lista è vuota → comportamento identico a prima
(safe). cargo resta incrementale: ricompila solo il crate cambiato.

Verifica: `find` vede 16 sorgenti; dopo `touch gui-core/src/lib.rs`,
`make -n build/gui.cwasm` schedula cargo+precompile (prima diceva "up to date").

## File toccati
- Makefile (RUOS_DESKTOP_SRCS + prereq della regola build/gui.cwasm)
