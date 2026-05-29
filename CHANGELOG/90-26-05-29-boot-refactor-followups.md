# 90 — Followups tracciati boot refactor

**Data:** 2026-05-29

## Cosa

Creato `docs/followups/boot-refactor.md` con 7 followup non-blocking
dal whole-implementation review:

- F1 🟢 banner SHA stale (build.rs cache).
- F2 🟡 banner content padding cosmetic.
- F3 🟡 test-boot non check QEMU exit.
- F4 🟢 executor "up" log non strutturato.
- F5 🟡 Unicode box chars in fb font (pre-Step-13).
- F6 🟢 get_acpi_info clona Vec ad ogni read.
- F7 🟢 IDT exception handlers stay kprintln by-design.

## Perché

Boot refactor approvato APPROVE WITH FOLLOWUPS. Items minori, non
blocking Step 12.

## File toccati

- docs/followups/boot-refactor.md (nuovo)
- CHANGELOG/90-26-05-29-boot-refactor-followups.md (nuovo)
