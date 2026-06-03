# 217 — docs/ARCHITECTURE.md + link dal README

**Data:** 2026-06-03

## Cosa
Aggiunto `docs/ARCHITECTURE.md`: walkthrough top-to-bottom di come funziona
ruos — HW (CPU/SMP, interrupt, ACPI, PCIe, storage, net, USB, console),
kernel (memoria, executor async cooperativo, VFS/PTY/pipe, proc/service, RNG,
klog), runtime WASM + ABI host (wasmi, fiber, fuel, namespace
`wasi_snapshot_preview1` + `ruos`), userland (shell, tool, SSH, self-install),
modello di concorrenza, ciclo di vita di un comando, source map. README: nuovo
link "How it all works" al doc.

## Perché
Richiesto un singolo file che spieghi l'intero SO (HW + kernel + OS),
linkato dal README. Mancava una vista d'insieme dell'architettura.

## Note
Dettagli verificati dal codice, non dai changelog: heap = 16 MiB (non 4 — la
docstring in memory è stale), fuel slice = 2_000_000_000, 25 host fn WASI + 33
`ruos`, ordine fasi di boot da `boot/mod.rs`.

## File toccati
- docs/ARCHITECTURE.md (nuovo)
- README.md (link al doc)
- CHANGELOG/217-26-06-03-architecture-doc.md (questo file)
