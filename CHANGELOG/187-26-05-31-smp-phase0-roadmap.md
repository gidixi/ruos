# 187 — SMP Fase 0 complete: roadmap updated, regressions green

**Data:** 2026-05-31

## Cosa
Marcata SMP Fase 0 come completata nel roadmap (`docs/superpowers/roadmap-rust-os.md`).
Aggiunta una sottosezione "Fase 0 — fondamenta per-CPU (✅ DONE)" sotto Step 18
che documenta i deliverable di Tasks 1-5:

- `IrqMutex<T>` — lock primitivo IRQ-safe (CHANGELOG 182)
- Per-CPU data via GS-base: `PerCpu`, `this_cpu()`, `init_bsp`, MAX_CPUS=16 (CHANGELOG 183)
- Per-core GDT/TSS + double-fault IST arrays, `gdt::init(cpu_id)`, BSP slot 0 (CHANGELOG 184)
- Enumerazione CPU via ACPI MADT — AP rilevati, NON avviati (CHANGELOG 185)
- Lock audit completo (~52 siti, zero MUST-FIX) + invariante executor documentato (CHANGELOG 186)

La nota specifica esplicitamente: tutto su 1 CPU, nessun AP avviato, executor
resta single-core (invariante documentato in `executor/mod.rs`). Indica Fase 1
(AP bring-up: INIT-SIPI-SIPI, per-CPU LAPIC timer) e Fase 2 (executor SMP-safe)
come lavoro rimanente.

Riferimenti inline a spec e audit:
- `docs/superpowers/specs/2026-05-31-smp-phase0-percpu-design.md`
- `docs/superpowers/notes/2026-05-31-smp-lock-audit.md`

Eseguita regressione completa (4 suite), tutti verdi:
- `make run-test`      → TEST_PASS
- `make run-ssh-test`  → TEST_PASS_SSH
- `make run-pipe-test` → TEST_PASS_PIPE
- `make run-fuel-test` → TEST_PASS_FUEL

(run-test: primo tentativo flake per timeout 120s imposto; secondo tentativo
con timeout completo 240s → TEST_PASS.)

## Perché
Task 6 (finale) di SMP phase-0: chiudere formalmente la fase documentando i
deliverable nel roadmap e verificando che le fondamenta per-CPU non abbiano
rotto nessuna delle suite di regressione esistenti.

## File toccati
- docs/superpowers/roadmap-rust-os.md (aggiunta sottosezione Fase 0 sotto Step 18)
- CHANGELOG/187-26-05-31-smp-phase0-roadmap.md (new)
