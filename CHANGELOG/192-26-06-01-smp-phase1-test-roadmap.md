# 192 — SMP Fase 1: test integrazione + roadmap aggiornata (Task 5)

**Data:** 2026-06-01

## Cosa
- Aggiunto `tests/smp-test.sh`: integration test headless QEMU `-smp 4` che
  asserisce `smp: 3/3 APs online` + assenza `#PF` entro 60 secondi. Include
  il disco AHCI se presente (boot completo fino a `init.sh complete`), cade
  back a no-disk altrimenti (l'asserzione SMP scatta nella fase interrupts,
  ben prima dello storage).
- Aggiunto target `run-smp-test: iso $(DISK_IMG)` nel Makefile (dopo
  `run-fuel-test`), con voce in `.PHONY`.
- Aggiornato `docs/superpowers/roadmap-rust-os.md` — Step 18, sezione Fase 1:
  aggiunta subsection "Fase 1 — AP bring-up → idle (✅ DONE)" con deliverable
  dettagliati (MpRequest, idt::load su AP, LAPIC-based cpu_id, bringup
  coordinator, ap_entry, test), link alla spec, evidenza VBox.

## Perché
Task 5 (finale) di SMP Fase 1: formalizzare il test di accettazione e
documentare lo stato done. Verificato su QEMU -smp 4 (TEST_PASS_SMP) e
VirtualBox con 4 vCPU (banner sha f33d286 == HEAD, 3/3 APs online, init.sh
complete, nessun #PF). Regressioni tutte verdi: TEST_PASS, TEST_PASS_SSH,
TEST_PASS_PIPE, TEST_PASS_FUEL.

## File toccati
- tests/smp-test.sh  (nuovo)
- Makefile  (run-smp-test target + .PHONY)
- docs/superpowers/roadmap-rust-os.md  (subsection Fase 1 done)
- CHANGELOG/192-26-06-01-smp-phase1-test-roadmap.md  (questo file)
