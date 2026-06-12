# 475 — unpack_bin: scrittura tmpfs by-move (no doppia copia per blob grossi)

**Data:** 2026-06-12

## Cosa

Nuovo path di scrittura tmpfs che **sposta** (move) il buffer invece di copiarlo,
usato dalla fase di boot `unpack_bin`:

1. `kernel/src/vfs/tmpfs.rs`: `Tmpfs::create_file_owned(path, content: Vec<u8>)`
   — crea un file regolare assumendo OWNERSHIP del `Vec` (zero copia), sync,
   parent deve esistere.
2. `kernel/src/vfs/mod.rs`: `vfs::write_file_owned(path, content: Vec<u8>)` —
   fast-path tmpfs via `create_file_owned`; fallback buffered (open+write+close)
   per mount non-tmpfs (FAT32 `/mnt`).
3. `kernel/src/boot/phases/unpack_bin.rs`: il membro decompresso viene passato a
   `write_file_owned` (move) invece di `write_file(&data)` (copia). Rimosso il
   secondo probe `heap_has(data.len())` per la "tmpfs copy" — non serve più.

## Perché

Il viewer (app GUI esterna, `.cwasm` ~74 MiB per via di rustls/TLS + epoch
codegen) NON veniva caricato nel launcher. Causa root, finalmente dal log:

```
WARN unpack_bin viewer.cwasm: skipped — no contiguous 74 MiB of heap for tmpfs copy
INFO unpack_bin unpacked 70 bins from bin.bgz (1 failed)
```

`unpack_bin` decomprimeva il membro (inflate → `data`, 74 MiB, OK: un blocco
contiguo c'era) ma poi `write_file` lo COPIAVA in un secondo `Vec` contiguo
(`TmpfsFile::write` → `content.resize` + `copy_from_slice`) mentre `data` era
ancora vivo → picco **2× 74 = 148 MiB** contigui simultanei, impossibili
nell'heap da 384 MiB già parzialmente occupato dagli altri ~70 bin → skip →
viewer mai in `/bin` → mai probato → assente dal launcher. Niente a che vedere
con tunable/epoch/copy nel drop-folder (i depistaggi precedenti).

Con la move il blob sta nell'heap UNA volta sola: il picco torna a 74 MiB (già
dimostrato disponibile dall'inflate riuscito), indipendente dal rounding
dell'allocatore (un semplice bump di `HEAP_SIZE` sarebbe stato fragile).

## File toccati
- kernel/src/vfs/tmpfs.rs
- kernel/src/vfs/mod.rs
- kernel/src/boot/phases/unpack_bin.rs
