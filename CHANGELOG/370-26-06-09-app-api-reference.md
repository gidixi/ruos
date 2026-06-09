# 370 — docs/api/: manuale API app-facing (stile crate docs) + regola auto-update

**Data:** 2026-06-09

## Cosa

- **`docs/api/`** (nuovo): manuale dell'API host che le app/tool WASM possono
  importare, **una pagina per modulo** (stile docs.rs), che cresce man mano:
  - `README.md` — indice, runtime, convenzioni, regola di manutenzione.
  - `ruos-window.md` — **pagina primaria**: l'API safe che l'app-author usa
    (`frame_once`, `WindowState`, `declare_manifest!`, helper, `RuosTermIo`),
    estratta da `ruos-desktop/crates/ruos-window/src/lib.rs`. È la fonte di verità
    per scrivere un'app senza leggere il kernel.
  - `wm.md` / `sys.md` / `term.md` — GUI Wasmtime (`.cwasm`): window manager,
    telemetria, terminale. Precise (signature `extern "C"`, layout blob, codici
    evento `poll_event`, errno).
  - `ruos.md` — tool wasmi (`.wasm`): modulo `ruos` (exec/fs, hw-enum, sysinfo,
    net, disk, time/power, tty, service, smp).
  - `wasi.md` — `wasi_snapshot_preview1` (lifecycle/clock/random/fd/path/sock) +
    nota sulla risoluzione path/cwd.
  - `wit.md` — component model (`gfx`/`power`/`term`, bringup).
  Popolato estraendo le registrazioni `func_wrap` del kernel + extern di
  `ruos-window` + `wit/` + i codici evento da `gui-core::abi`.
  (Sostituisce il precedente file singolo `docs/app-api.md`, rimosso.)
- **Regola in `CLAUDE.md`** aggiornata → `docs/api/`: ogni host fn app-facing
  aggiunta/rimossa/modificata (`func_wrap("wm"|"sys"|"term"|"ruos", …)` in
  `kernel/src/wasm/wt/*` o `host/*`, o `wit/*.wit`) va documentata nella pagina
  corrispondente nello STESSO commit (entry + "Last reviewed"; per le GUI anche
  l'`extern "C"` in `ruos-window`).
- **`demo-apps-sdk/bootstrap.ps1`** (gitignored): copia l'intera `docs/api/` in
  ogni progetto generato come `api/` (refresh a ogni run, param `-RuosRoot`) →
  manuale offline che viaggia col progetto indipendente.

## Perché

I progetti app creati con la SDK sono indipendenti (fuori dal repo SO): serve un
manuale preciso e navigabile (non un singolo blob) dell'API del SO, disponibile
offline e che si aggiorna da solo man mano che il kernel espone nuove host fn.
Struttura per-modulo = precisa, manutenibile, estendibile; una regola CLAUDE ne
impone l'update; la SDK la copia nei progetti.

## Verifica

`bootstrap.ps1 -Path <proj>` → `<proj>\api\` con 7 pagine (README + wm/sys/term/
ruos/wasi/wit).

## File toccati
- docs/api/{README,ruos-window,wm,sys,term,ruos,wasi,wit}.md (nuovi); docs/app-api.md rimosso
- CLAUDE.md
- demo-apps-sdk/bootstrap.ps1, demo-apps-sdk/README.md (gitignored)
