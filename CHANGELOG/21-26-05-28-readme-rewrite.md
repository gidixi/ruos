# 21 — README riscritto per il progetto Rust OS

**Data:** 2026-05-28

## Cosa

Sostituito il `README.md` originale del TP-Arqui (spagnolo, x64BareBones) con
un README nuovo per `ruos`: descrizione del progetto, tabella stato della
roadmap (1-4 ✅, 5 next), prerequisiti (Ubuntu/WSL packages + rustup nightly),
istruzioni `make iso` / `make run-test` / `make run`, boot da USB via `dd`,
layout del repo, link a roadmap/spec/plan/changelog, licenza GPL v3.

## Perché

Il README precedente era del progetto originale (x64barebones, ITBA TP); dopo
la rimozione dell'albero C non aveva più senso. Un README chiaro e in inglese
descrive lo stato corrente, le istruzioni di build/test e il boot su HW reale,
allineato col contenuto del repo pubblico su GitHub.

## File toccati

- README.md
- CHANGELOG/21-26-05-28-readme-rewrite.md
