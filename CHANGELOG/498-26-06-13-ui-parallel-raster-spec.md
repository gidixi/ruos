# 498 — Spec: rasterizzatore UI software parallelo (GPU-less)

**Data:** 2026-06-13

## Cosa
Spec di design del sotto-progetto #1 della direzione "fluidità UI come Ubuntu, in
Rust": rasterizzatore software **a bande parallelo** (stile llvmpipe) per la UI egui,
GPU-less. Documenta il pivot dal driver GPU Intel (Gen12 Xe-LP del target reale =
montagna pluriennale, solo HW), il prior-art già in tree (compositing SMP, double
buffer, idle-skip, damage-driven raster, dormienza finestre — tutto ✅), il profiling
baseline `wm-fps` (TCG, solo rapporti: raster ≈ 3-4.5× il present, single-thread per
finestra), e le leve: **#1 raster band-parallel** (cuore), **#0 repaint scheduling**
(polish idle), #2 damage-raster (già fatto, riuso), #3 SIMD (futuro). Include la
premessa critica da validare per prima (rayon parallel-for cooperativo dentro
`frame()`), il design per-banda puro che rispetta la Regola d'oro di `gui-core`, e i
criteri di successo (speedup `frame_all` multi-core, equivalenza seriale↔banda
bit-identica). Target perf assoluti da riempire con numeri HW reali.

## Perché
La richiesta era un driver GPU per accelerare la UI. Analisi: il target reale
(i7-11800H, iGPU `8086:9A60` Tiger Lake Gen12 Xe-LP) rende un driver 3D nativo
impraticabile (solo HW reale, ISA Xe-LP); e il costo UI dominante è la
rasterizzazione, non il compositing. Ubuntu è "fluido senza GPU" grazie a un
rasterizzatore software multi-thread (llvmpipe) + retained-mode + damage tracking —
architettura che ruos ha già quasi tutta, tranne il raster parallelo per-finestra.

## File toccati
- docs/superpowers/specs/2026-06-13-ui-parallel-raster-design.md (nuovo)
- CHANGELOG/498-26-06-13-ui-parallel-raster-spec.md (nuovo)
