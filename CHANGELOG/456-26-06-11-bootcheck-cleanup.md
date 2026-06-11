# 456 — Pulizia batteria boot-checks: via gate/viewer embedded e check superati

**Data:** 2026-06-11

## Cosa

Rimossi dalla batteria boot-checks i check obsoleti o insostenibili:

- **Gate Blitz (`viewer-gate.cwasm`, 55 MB) + viewer embedded (`viewer.cwasm`,
  77 MB)**: erano blob AOT Stylo embedded nel kernel SOLO per i boot-check.
  Dopo il flip `epoch_interruption` (455) i due blob ri-AOTati esaurivano i
  frame a `-m 1024` → `#PF` irrecuperabile a metà bench. Il gate Phase-0.5 ha
  già svolto il suo ruolo (numeri archiviati in CHANGELOG 425 e
  `ruos-test/apps/viewer-gate/GATE.md`); il viewer REALE vive in `apps/` ed è
  coperto dal manifest-scan del launcher + test visivo.
- **`wasip1-probe`** (blob + entry APPS + self-test): superato da egui-demo
  (stesso path std/wasip1 reactor + commit, più completo).
- **Reactor spike** (`run_reactor_spike`, 5 frame su istanza raw): superato da
  lifecycle + watchdog self-test che passano dal path di spawn reale.
- **`gfxtest.cwasm`** (smoke ABI `ruos_gfx`): apparteneva al modello
  pre-compositor `gui.cwasm`, ritirato col pivot Model A (i moduli linker
  gfx/gui per run_cwasm restano registrati).
- Tool sorgenti rimossi: `tools/wt-gfxtest/`, `tools/wt-wasip1-probe/`.
  Allineati Makefile (regole + prereq iso/test-boot), `build-iso.ps1`,
  `WT_KCWASMS` (ora include `spin_reactor.cwasm`).

Blob embedded boot-checks: da ~132 MB (gate+viewer) a ~21 MB totali (max:
shell 10 MB, egui_demo 9 MB).

Batteria residua: hello, echo, cat, spin (SMP exec), component bringup, sp3
logic, registry, lifecycle, egui-demo, spc, spd, **watchdog epoch** (nuovo,
455), demand-paging, console, rng per-core.

## Perché

Il `#PF` durante il bench gate (boot-checks post-455) era esaurimento frame:
i blob test crescevano ad ogni feature (rustls +14 MB, epoch +9 %) e
caricavano in RAM 132 MB ×2 (immagine kernel + copie exec nel MODULE_CACHE)
solo per provare cose già coperte altrove. Nota: con la rimozione del gate
sparisce anche il termometro dell'overhead epoch in batteria — la misura
resolve/paint resta disponibile nella status bar del viewer reale
(`make run` / HW).

## File toccati

- kernel/src/wasm/wt/{mod.rs,wm.rs}
- kernel/src/boot/phases/{interrupts.rs,fs.rs}
- Makefile, build-iso.ps1
- tools/wt-gfxtest/ (rimosso), tools/wt-wasip1-probe/ (rimosso)
- kernel/src/wasm/wt/{viewer,viewer-gate,probe,gfxtest}.cwasm (rimossi)
- tools/wt-precompile/Cargo.toml (commento)
