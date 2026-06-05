# 292 — Ripristino path di build locale in CLAUDE.md

**Data:** 2026-06-05

## Cosa
La PR #2 (sync doc) aveva impostato l'"Ambiente di build" di CLAUDE.md sui path di
un altro PC (`/mnt/w/Work/GitHub/ruos`, WSL `Ubuntu-22.04`). Ripristinati ai path di
questa macchina: repo `/mnt/e/MinimalOS/BasicOperatingSystem`, WSL `Ubuntu`
(distro/path usati e verificati in tutta la sessione). Tenuti tutti gli altri
miglioramenti doc della PR.

## Perché
Seguire CLAUDE.md alla lettera coi path dell'altro PC farebbe fallire i build qui.

## File toccati
- CLAUDE.md
