# 419 — Cache manifest del launcher keyed su (path, size)

**Data:** 2026-06-10

## Cosa

Lo scan del catalogo app del compositor (`scan_apps`, ~1 Hz) non ri-proba più
ogni `.cwasm` presente: il memo `MANIFEST_CACHE` ora salva per ogni stem la
identità del file risolto (`path` + `size` da `vfs::stat`, solo metadata) oltre
al risultato del probe. A ogni scan la LISTA file si rilegge come prima
(readdir + stat per file — economico, hot-plug `/mnt/apps` invariato), ma il
probe costoso (deserialize AOT multi-MB + instantiate throwaway + `manifest()`
+ drop, con relativa tempesta di mprotect/TLB-shootdown) parte SOLO per file
nuovi, con size cambiata o shadow-flip `/bin`↔`/mnt/apps`. File spariti →
entry evitta dal memo e dal catalogo. In più il lock `IrqMutex` del memo non è
più tenuto attraverso le chiamate bloccanti (VFS read / instantiate), e un
probe fallito (es. copia parziale nella drop-folder) viene ri-tentato quando la
size si assesta invece di restare memoizzato per sempre.

## Perché

Parte 4 del fix "TLB shootdown storm" (spec
`docs/superpowers/specs/2026-06-10-tlb-shootdown-batch-design.md`): il primo
scan del launcher deserializzava+istanziava+droppava ~54MB di `.cwasm` (≈33-36k
shootdown broadcast) e il lavoro si ripeteva a ogni refresh per file già visti.

## File toccati

- kernel/src/wasm/wt/wm.rs
