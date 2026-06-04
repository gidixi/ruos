# 257 — Wasmtime WASI file ops: run real `cat`

**Data:** 2026-06-04

## Cosa
Estesa l'impl WASI Preview 1 su Wasmtime con le operazioni su file, eseguite
sincronamente via `crate::vfs::block_on` (le future tmpfs completano in un poll):
- `WtState` ora ha una tabella fd (`WtFd::{Console,Vfs,Closed}`, preopen "/" a fd 3);
- aggiunte `fd_read`, `fd_seek`, `fd_close`, `fd_fdstat_get`, `fd_filestat_get`,
  `fd_prestat_get`, `fd_prestat_dir_name`, `path_open` (oltre alle 6 core).
- Boot-check fase fs: semina `/wt-cat-test.txt`, poi esegue `cat.cwasm` reale →
  stampa `CAT-OK-MARKER`, exit=0.

Verificato in QEMU: `cat` (vero binario wasip1) apre e legge un file tmpfs via
WASI e lo stampa. Nota: il demo va in fase `fs` (il VFS si monta lì; la fase
`interrupts` è troppo presto). Avvio cat ~1.8–3s (deserialize+instantiate del
cwasm 401KB) — costo una-tantum, non per-frame.

## Perché
Completa la base WASI così che tool `.wasm` veri che leggono file girino su
Wasmtime. (Tool come `ls` usano host fn `ruos` custom, non WASI readdir →
fuori da questo set.) Verso il router shell `.cwasm` e la GUI.

## File toccati
- kernel/src/wasm/wt/{state.rs,wasi.rs,mod.rs}
- kernel/src/wasm/wt/cat.cwasm (artefatto test)
- kernel/src/boot/phases/fs.rs, kernel/src/boot/phases/interrupts.rs
