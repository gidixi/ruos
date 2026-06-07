# 321 — Fix: abilitare SSE/SIMD sugli AP (compositor su GUI-core non crashava più)

**Data:** 2026-06-07

## Cosa

`cpu/ap.rs::ap_entry` ora abilita SSE/SIMD su ogni AP (chiama
`crate::boot::phases::arch::enable_simd()`, resa `pub(crate)`) e switcha il core su
uno stack heap da 8 MiB prima di entrare nel worker.

Prima: gli AP partivano con SIMD disabilitato (CR0.EM set, CR4.OSFXSR clear) — solo
il BSP abilitava SSE/AVX in `arch::init`. La prima istruzione SSE del codice AOT
cranelift float (rendering egui sul core GuiCompositor) faceva `#UD` e uccideva il
core 1.

## Perché

Testando la ISO su VirtualBox, lanciando il compositor il core 1 (GUI) moriva
(`#UD at rip=0xFFFFD00000152161`, poi `mute cores=1`) mentre SSH/BSP restavano vivi
(isolamento shared-nothing OK). Riprodotto in QEMU: `-smp 4` (compositor sul core 1)
crasha su `-cpu max` E `Haswell` (quindi NON una feature CPU mancante); `-smp 1`
(compositor sul BSP) non crasha → bug AP-specifico. Le app intere
(spin/echo/parallel-exec) non toccano SSE → il bug era mascherato, e
`run-ssh-gui-test` verifica solo handoff+SSH (sul BSP), mai la longevità del core 1.
Conseguenza: il compositor sul core dedicato (Step 5) non era mai sopravvissuto oltre
il primo frame.

Verificato dopo il fix: `compositor handed off to gui core 1` + `all 4 cores alive`
sostenuto (core 1 vivo), niente #UD; `parallel-exec overlap=true`, `pty-route`,
`run-test`, `run-exec-ap-test` restano verdi.

Nota minore: 2 `mute cores=1` transitori al primo render (egui blocca il core >2
tick → il battito salta, poi recupera) — da tunare quando si fa il 6-recover.

## File toccati

- kernel/src/cpu/ap.rs
- kernel/src/boot/phases/arch.rs
- CHANGELOG/321-26-06-07-ap-sse-simd-fix.md
