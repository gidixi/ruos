# 464 — Spec multi-tenant hardening (epoch watchdog, Spectre, quote, PKU)

**Data:** 2026-06-10

## Cosa

Nuova spec di design `docs/superpowers/specs/2026-06-10-multi-tenant-hardening-design.md`:
piano in 5 sottoprogetti per preparare ruos a codice WASM di terzi e
multi-user, nel rispetto dei vincoli confermati (no ring 3, no page table
per-istanza, no PKS obbligatorio):

- SP1: epoch watchdog Wasmtime (deadline su `frame()` compositor + callback
  Ctrl-C sui tool `.cwasm`; config hashata → ricompilare i `.cwasm`).
- SP2: audit mitigazioni Spectre Cranelift (fissare i flag, verificare
  pagina 0 non mappata) + audit superfici host fn Wasmtime.
- SP3: ResourceLimiter/quote sugli store Wasmtime.
- SP4: PKU U=1 feature-detect (sketch, spec dedicata futura).
- SP5: multi-user VFS (fuori scope, spec futura).

## Perché

Dichiarato l'obiettivo futuro "codice di terzi + multi-utente": il threat
model cambia, il sandboxing solo-software in ring 0 va indurito. La spec
fotografa lo stato verificato nel codice (demand paging già presente, fuel =
watchdog kill, nessuna epoch interruption, `frame()` senza deadline) e fissa
priorità e gating.

## File toccati

- docs/superpowers/specs/2026-06-10-multi-tenant-hardening-design.md (nuovo)
