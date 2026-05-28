# 20 — Rimozione albero C legacy + cleanup repo

**Data:** 2026-05-28

## Cosa

- **Rimossa intera cartella `x64barebones/`** (vecchio kernel C su Pure64,
  Bootloader, Userland, Image, scripts). Vive come riferimento storico in git
  history fino al commit `c1d2a81`.
- **CLAUDE.md** aggiornato: sezione "Stato" descrive il codice attivo Rust e
  l'archiviazione del C; step 1-4 della roadmap marcati ✅ FATTO; rimosso il
  paragrafo "Legacy C" dall'ambiente di build.
- **`docs/superpowers/roadmap-rust-os.md`** aggiornato per riflettere la
  rimozione (riferimenti a `x64barebones/Toolchain/` ecc. corretti).
- **`.gitignore`** ripulito: rimossi i glob legacy `*.bin`/`*.sys`/`*.img`/
  `*.qcow2`/`*.vmdk`/`*.o` (servivano al build C, ora rischiavano di mascherare
  binari intenzionali); tenuti i path Rust (`build/`, `kernel/target/`,
  `third_party/limine/`); aggiunte `.vscode/`, `.DS_Store`, `Thumbs.db`.

## Perché

Il C era stato declassato a riferimento dopo il pivot a Rust+Limine; con i
milestone Rust che avanzano (heap fatto) il codice C non serve più nel working
tree. La history lo preserva per consultazione.

## File toccati

- x64barebones/ (rimossa)
- CLAUDE.md
- docs/superpowers/roadmap-rust-os.md
- .gitignore
- CHANGELOG/20-26-05-28-remove-legacy-c-tree.md
