# 85 — Spec: Boot phase refactor (pro-grade kmain + structured log)

**Data:** 2026-05-29

## Cosa

Scritta spec in `docs/superpowers/specs/2026-05-29-rust-boot-refactor-design.md`.

Goal: kmain da 240 a ~30 righe. 6 phases esplicite (arch/mem/interrupts/
devices/fs/userland). Logger strutturato (`[T+SECS.MILLISs] L mod: msg`).
Banner ASCII con version + git SHA + build date. Smoke test gated dietro
`boot-checks` feature.

Decomposizione 3 task:
1. Infra: boot/{mod,log,banner,error}.rs + build.rs (RUOS_GIT_SHA).
2. Phases extraction: split kmain in phases/*.rs.
3. Cleanup + final log migration + `make test-boot` target.

Sentinel `shell: init.sh complete` resta invariato; refactor non cambia
behavior osservabile dei wasm.

## Perché

User feedback: "rendere fase di boot piu organizata e pro non hobby os".
Polish architetturale prima di Step 12 (PTY).

## File toccati

- docs/superpowers/specs/2026-05-29-rust-boot-refactor-design.md (nuovo)
- CHANGELOG/85-26-05-29-boot-refactor-spec.md (nuovo)
