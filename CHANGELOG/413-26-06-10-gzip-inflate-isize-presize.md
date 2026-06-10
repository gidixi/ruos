# 413 — gzip-core: inflate pre-dimensionato da ISIZE (fix OOM unpack bin.bgz)

**Data:** 2026-06-10

## Cosa
`gzip-core::format::inflate_raw` ora pre-alloca il buffer di output ESATTAMENTE
alla dimensione decompressa (ISIZE del trailer gzip), con crescita additiva
(+256 KiB) solo se l'hint è errato e clamp a 64× l'input come guardia. Prima
allocava `Vec::with_capacity(input*4)` e poi `resize(capacity*2)` alla prima
iterazione = `input*8`. `decompress` passa l'hint ISIZE letto dal trailer.
Aggiornato anche il log/commento stale in `fs.rs` (`bin populate deferred to
unpack_bin phase`).

## Perché
Su hardware reale `unpack_bin` panicava con `memory allocation of 168627888
bytes failed` (~21 MB compresso × 8) decomprimendo un membro `.cwasm` grosso:
il raddoppio del buffer allocava ~8× l'input e, con la copia di realloc + i
file già in tmpfs + frammentazione talc, sforava lo heap da 384 MiB. Il
pre-dimensionamento da ISIZE porta il picco di un singolo membro da ~168 MB
alla sua dimensione reale (es. `viewer` 61 MB). I 23 test di gzip-core restano
verdi; build no_std ok.

## File toccati
- user/gzip-core/src/format.rs
- kernel/src/boot/phases/fs.rs
- CHANGELOG/413-26-06-10-gzip-inflate-isize-presize.md
