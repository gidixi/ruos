# 175 — WASI fd_readdir Task 5: docs

**Data:** 2026-05-31

## Cosa
Aggiornata la documentazione per riflettere che `fd_readdir` è atterrato
([[171-26-05-31-fdentry-dir]] – [[174-26-05-31-readdir-smoke]]).

- `README.md`: nota nella sezione Status che la compatibilità WASI cresce
  incrementalmente; `fd_readdir` esportato → `std::fs::read_dir` e crate
  `walkdir`-style funzionano da binari `wasm32-wasip1` `std` puri; il
  legacy `ruos.readdir` resta per i tool esistenti.
- `docs/superpowers/roadmap-rust-os.md`: nuova sezione "Compatibilità
  WASI — incrementale (in corso)" dopo Step 16, che documenta il primo
  item (`fd_readdir`) con riferimento alla spec e ai changelog 171-175.

## Perché
Chiusura del piano della spec (Task 5). I contributori che leggono
README/roadmap vedono che `std::fs::read_dir` ora funziona e che esiste
un filone "WASI compat" trasversale.

## File toccati
- README.md
- docs/superpowers/roadmap-rust-os.md
