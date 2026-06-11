# 441 — init: host ABI ruos.unit_* + timer_list + doc API

**Data:** 2026-06-11

## Cosa
- `wasm/host/unit.rs` (nuovo): `unit_list`, `unit_status`, `unit_start`,
  `unit_stop`, `unit_enable`, `timer_list`, `unit_reload` — TSV
  buffer+used (8 ENOBUFS), pattern di `host/service.rs`. Wired in
  `host/mod.rs::install`.
- `service/mod.rs`: `list_tsv`/`status_tsv` (10 campi) + `timers_tsv`
  (schedule ri-serializzato umano + fire raw).
- `docs/api/ruos.md`: nuova sezione "Unit manager" con tabella errno +
  funzioni; Last reviewed 2026-06-11 (regola CLAUDE.md: stesso commit).

## Perché
Fase 8 spec init-units: superficie ABI per il CLI `unitctl`.

## File toccati
- kernel/src/wasm/host/unit.rs
- kernel/src/wasm/host/mod.rs
- kernel/src/service/mod.rs
- docs/api/ruos.md
