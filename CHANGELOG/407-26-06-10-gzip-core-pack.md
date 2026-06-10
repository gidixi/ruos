# 407 — gzip-core: modulo pack (container RBIN)

**Data:** 2026-06-10

## Cosa
Implementato `user/gzip-core/src/pack.rs`: container "RBIN" che raccoglie più file
come membri gzip indipendenti. API pubblica:
- `write_archive(entries, level) -> Vec<u8>` — costruisce l'archivio comprimendo
  ogni file separatamente.
- `parse(data) -> Result<ArchiveIter, PackError>` — valida magic/version e ritorna
  un iteratore `(name, gz_member)` senza decomprimere.
- `decompress_member(gz) -> Result<Vec<u8>, GzError>` — decomprime un singolo membro.
- `PackError` — `BadMagic`, `BadVersion`, `Truncated`.

Aggiunto `#[derive(Debug)]` su `ArchiveIter` richiesto dai bound di
`Result::unwrap_err` nei test. 5 test unitari tutti verdi; suite completa 23/23.

## Perché
Task 2 del piano `bin-pack + livecd`: il kernel usa RBIN per caricare i binari WASM
dall'immagine LiveCD decomprimendo un file alla volta (peak heap basso).

## File toccati
- user/gzip-core/src/pack.rs
