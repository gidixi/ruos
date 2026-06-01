# 198 — run-smp2-test + roadmap SMP Fase 2 done

**Data:** 2026-06-01

## Cosa
Aggiunto test di integrazione `run-smp2-test` che verifica il parallel compute
pool di SMP Fase 2. Il test avvia QEMU `-smp 4` con disco e rete, si connette via
SSH, esegue `smptest` e asserisce `speedup >= 1.5x` e `>= 2 core distinti`.

Risultato su QEMU: `parallel=152ms sequential=506ms speedup=3.32x cores=[1,2,3]`
→ `TEST_PASS_SMP2`.

Aggiornato il roadmap con la sottosezione "Fase 2 — kernel compute offload pool
(✅ DONE)" nello Step 18 SMP (stile italiano coerente con Fase 0/1): descrive la
SMP-safe work queue, la host fn `ruos_smp_bench`, il tool `smptest`, lo speedup
reale misurato e il test di integrazione.

VirtualBox verify (6 vCPU): banner sha `0f1bd62` == HEAD, `5/5 APs online`,
`init.sh complete`, nessun `#PF` — il busy-poll AP worker loop non causa fault.

Full regression: TEST_PASS, TEST_PASS_SSH, TEST_PASS_PIPE, TEST_PASS_FUEL,
TEST_PASS_SMP — tutti verdi.

## Perché
Task 5 (finale) di SMP Fase 2: test osservabile + documentazione che chiude il
ciclo spec→impl→verifica del compute pool parallelo.

## File toccati
- tests/smp2-test.sh (nuovo)
- Makefile (target run-smp2-test)
- docs/superpowers/roadmap-rust-os.md (sottosezione Fase 2)
- CHANGELOG/198-26-06-01-smp2-test-roadmap.md (questo file)
