# 378 — Desktop app sleep/wake — integrazione

**Data:** 2026-06-09

## Cosa
Feature completa "app sleep/wake" del desktop (compositor kernel-side): le finestre
idle dormono (la loro `frame()` non viene chiamata), si svegliano su input
(focus/hover/click/tastiera), override `wm.stay_awake()` o output PTY legato. Il PTY
watchdog non uccide più i terminali GUI locali (solo i pair SSH leak-ati); il pair di
un terminale è liberato alla chiusura della finestra. Bump del pointer submodule
ruos-desktop (bindings `stay_awake`/`wake_on_pty` + auto-bind terminale +
system-app `stay_awake`).

Vedi spec `docs/superpowers/specs/2026-06-09-desktop-app-sleep-design.md` e piano
`docs/superpowers/plans/2026-06-09-desktop-app-sleep.md`.

## Perché
I terminali idle venivano uccisi dal watchdog (perdita sessione + scrollback) e tutte
le app giravano `frame()` ogni giro (CPU sprecata sul GUI core). Ora le idle dormono e
si svegliano su interazione/dati; chi deve aggiornarsi sempre (monitor) fa opt-out.

## File toccati
- ruos-desktop (submodule pointer → 7cb8d82)
- (codice nei commit 368-377 + submodule)
