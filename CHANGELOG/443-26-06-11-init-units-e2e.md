# 443 — init: verifica e2e boot-checks + spec marcata implementata

**Data:** 2026-06-11

## Cosa
- Verifica QEMU: `make run-test CARGO_FEATURES=boot-checks` → tutti i 7
  gruppi `svc-check` verdi (yaml, json, schedule, unitfile, compute_next,
  topo, serialize) + TEST_PASS; build release (`make iso` senza feature)
  + smoke test PASS.
- Spec `2026-06-09-init-units-timers-design.md` marcata implementata.
- **Gotcha scoperto:** `run-test` ri-invoca `make iso` internamente —
  `make iso CARGO_FEATURES=boot-checks && make run-test` ri-builda il
  kernel SENZA la feature (TEST_PASS ingannevole, i check non girano).
  Forma corretta: `make run-test CARGO_FEATURES=boot-checks` (la var CLI
  propaga al sub-make). Verificare le righe `svc-check ... OK` nel log.

## Resta da fare (manuale, QEMU `make run` o HW)
Checklist Task 15 del piano: unit file in /mnt/etc/units + reload, timer
`every 10s`, daemon restart/backoff + `unitctl stop`, deps after/requires,
persistenza enable/disable al reboot.

## File toccati
- docs/superpowers/specs/2026-06-09-init-units-timers-design.md
