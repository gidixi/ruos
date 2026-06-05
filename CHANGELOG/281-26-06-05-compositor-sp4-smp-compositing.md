# 281 — Compositor SP4: compositing SMP-parallelo

**Data:** 2026-06-05

## Cosa
Il compositing per-frame del compositor (piano
`2026-06-05-compositor-sp4-smp-compositing.md`, contract decision 6) ora gira in
**parallelo sul compute-pool SMP** (gli AP).

- Nuovo kernel pixel puro `kernel/src/wasm/wt/compose.rs`: `WinDesc` (ptr + rect
  di UN footprint) + `composite_band(back, stride, screen_w, band_y0, band_y1,
  bg, &[WinDesc])` — pulisce la banda al colore di sfondo, poi painter's-algorithm
  copia per-riga l'overlap di ogni footprint nelle righe `[band_y0, band_y1)`.
  Nessuno stato globale → identico su BSP e AP.
- `Compositor::present` riscritto: il BSP costruisce i **footprint DECORATI** (via
  `compose_window` — title bar + [X] + testo + surface, NON le surface grezze →
  le decorazioni sopravvivono al raster parallelo), riempie un arena `static`
  `WIN_ARENA` di `WinDesc`, poi `dispatch_bands` divide lo schermo in bande
  orizzontali disgiunte (una per core online, cap `MAX_BANDS`), sottomette un job
  pure-CPU per banda a `crate::smp::pool` (`fn(&[u8])->u64` via `BAND_ARENA`
  `static`), fa il join di tutte (`poll_done`), e infine il BSP presenta il
  back-buffer con UN solo `gfx::blit` (che ricompone il cursore). Fallback inline
  per 1-CPU / pool pieno.
- Marker seriale one-shot `composite cores=N [...]` (bitset core via `cpu_id()`
  LAPIC-based) dopo 30 frame.
- Feature `serial-composite` (forza `n_bands=1`) per il test di equivalenza.

**Invarianti di concorrenza:** bande con righe disgiunte ⇒ nessuna race sul
back-buffer; gli arena `static` sono scritti SOLO dal BSP tra frame già joinati; i
footprint restano vivi nel BSP per tutta la durata dei job (drop dopo il join);
il framebuffer reale è scritto SOLO dal BSP, una volta.

## Perché
Il compositing è lavoro pure-CPU spalmabile sui core (spec §3.3/§6), sfruttando
il compute-pool Fase-2 esistente senza toccare l'executor cooperativo del BSP. Lo
scheduling degli `frame()` delle app resta seriale sul BSP — solo il raster va in
parallelo.

## Verifiche
- QEMU `-smp 4`: `wm composite cores=3 [1, 2, 3]` (≥2 core hanno composto bande;
  il BSP non prende bande inline quando >1 core è online — atteso).
- **Equivalenza**: `make run-comp-smp-test` → lo screendump della build parallela
  è **byte-identico** a quello della build `serial-composite` (`n_bands=1`),
  1280×800 — nessuna seam di banda; decorazioni intatte. `TEST_PASS_COMP_SMP`.
- Review finale (unsafe SMP, aliasing bande, lifetime footprint, join, contract):
  **pulita**.
- **VirtualBox (≥4 vCPU + EFI): VERIFICA MANUALE PENDENTE** (non eseguibile
  headless da qui). Atteso: `smp N/M APs online` + `composite cores=K (K≥2)` +
  finestre identiche a QEMU. Da fare prima del merge se si vuole copertura HW-like
  (memoria: testare VBox per modifiche CPU/MSR/STI-sensitive).

## File toccati
- kernel/src/wasm/wt/compose.rs (nuovo)
- kernel/src/wasm/wt/wm.rs
- kernel/src/wasm/wt/mod.rs
- kernel/Cargo.toml (feature serial-composite)
- tests/comp-smp-test.sh (nuovo)
- build/comp_shot.py (nuovo, driver screendump QMP)
- Makefile (run-comp-smp-test)
