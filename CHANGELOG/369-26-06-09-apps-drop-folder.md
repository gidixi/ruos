# 369 — apps/ drop folder: app .cwasm esterne auto-incluse nell'ISO

**Data:** 2026-06-09

## Cosa

Aggiunto un punto di integrazione generico per app GUI buildate **fuori dal repo
del SO**:

- **`apps/`** — cartella drop tracciata (`apps/README.md` + `apps/.gitkeep`); ogni
  `apps/*.cwasm` ignorato da git.
- **Makefile**: hook generico, app-agnostico, in entrambi i target ISO (`iso` e il
  build di `test-boot`):
  ```
  -cp apps/*.cwasm $(ISO_ROOT)/bin/ 2>/dev/null || true
  ```
  Copia ogni `.cwasm` droppata in `apps/` dentro `/bin`. Il compositor la scopre
  via scan `manifest()` → appare nel launcher. **Nessun cambio kernel/Makefile per
  nuova app.**
- **`.gitignore`**: `/apps/*.cwasm` + `/demo-apps-sdk/`.

Affiancato (gitignored, non nel repo SO): **`demo-apps-sdk/`**, SDK scheletro
standalone per buildare app GUI `.cwasm` — workspace + app template
`hello-window`, `new-app.ps1` (scaffolder interattivo), `build.ps1` (cargo
wasm32-wasip1 + wt-precompile), `deploy.ps1` (copia in `apps/` + `make iso`).

## Perché

Servire un toolchain per nuove app (prima fra tutte un browser) **senza toccare i
sorgenti del SO**: si builda altrove un `.cwasm`, lo si droppa in `apps/`, e
`make iso` lo include automaticamente. Disaccoppia *dove si builda* da *come si
integra*. La SDK usa i crate ABI da `ruos-desktop` e `wt-precompile` da `tools/`
via path (i tunables Wasmtime devono combaciare col kernel).

## Verifica

`cp <app>.cwasm apps/ && make iso` → `build/iso_root/bin/<app>.cwasm` presente.
SDK: `new-app.ps1` scaffolda un'app che compila; `build.ps1` produce
`deploy/<id>.cwasm` (6.6 MB per hello); `deploy.ps1` la droppa in `apps/`.

## File toccati
- apps/README.md (nuovo), apps/.gitkeep (nuovo)
- Makefile (hook apps/*.cwasm, 2 target)
- .gitignore
