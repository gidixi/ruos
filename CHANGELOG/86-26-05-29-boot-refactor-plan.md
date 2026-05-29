# 86 — Plan: Boot phase refactor (3 task)

**Data:** 2026-05-29

## Cosa

Plan scritto in `docs/superpowers/plans/2026-05-29-rust-boot-refactor.md`.
3 task TDD bite-sized:

1. **Boot infra**: build.rs (env vars git SHA + date), boot/{mod,log,
   banner,error}.rs, banner stamp in kmain. Init flow invariato.
   Sentinel `shell: init.sh complete` PASS.
2. **Phases extraction**: 6 phases (arch/mem/intr/dev/fs/userland),
   boot::run driver, kmain ridotto a ~25 righe, smoke gated dietro
   `boot-checks` feature.
3. **Log migration + test-boot**: tutti init-time `kprintln!("ruos: …")`
   migrati a `binfo!`, `make test-boot` target con feature on.

Numerazione changelog implementer: 87-89.

## Perché

Tradurre spec boot refactor in 3 commit incrementali con sentinel
sempre PASS.

## File toccati

- docs/superpowers/plans/2026-05-29-rust-boot-refactor.md (nuovo)
- CHANGELOG/86-26-05-29-boot-refactor-plan.md (nuovo)
