# 457 — unpack_bin: skip-on-OOM, un .cwasm gigante non uccide più il boot

**Data:** 2026-06-11

## Cosa

`unpack_bin` (bin.bgz → tmpfs /bin) ora sonda l'heap con `try_reserve_exact`
PRIMA dell'inflate (ISIZE dal tail gzip) e PRIMA della copia in tmpfs (mentre
il buffer inflate è ancora vivo → il picco transiente è 2× la size). Se manca
un blocco contiguo, il membro viene SALTATO con
`WARN unpack_bin <nome>: skipped — no contiguous N MiB of heap …` e il boot
prosegue (l'app manca da /bin, il sistema vive).

## Perché

Boot-loop su VirtualBox (e riprodotto in QEMU `-smp 4 -m 1024`):

```
KERNEL PANIC: memory allocation of 77423920 bytes failed
```

`apps/` conteneva 147 MB di `.cwasm` (viewer 77 + viewer-gate 55 + testapp +
calculator) bundlati in bin.bgz; l'alloc presize di `decompress_member` è
infallibile → OOM = panic = boot morto. Con SMP attivo la frammentazione
dell'heap (384 MiB) è peggiore che single-core: stessa ISO passava a 1 core e
moriva a 4. Un'app drop-folder opzionale non deve MAI essere fatale.

Nota operativa: a `-m 1024` + SMP il viewer (77 MB, picco 2×≈150 MB) viene
saltato. Rimedi: VM con ≥2 GiB di RAM, e/o alleggerire `apps/`
(viewer-gate.cwasm da 55 MB è un bench, non ha senso in /bin).

## File toccati

- kernel/src/boot/phases/unpack_bin.rs
