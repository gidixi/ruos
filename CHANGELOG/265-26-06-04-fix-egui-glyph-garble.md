# 265 — Fix: testo egui garbled in ruos (f32.floor/ceil via libcall no_std rotta)

**Data:** 2026-06-04

## Cosa
Risolto il glitch noto del testo egui reso **garbled solo in ruos** (perfetto su
PC / wt-harness con cwasm byte-identico). I caratteri ora sono nitidi.

## Root cause
egui/`ab_glyph` calcola il bounding box pixel di ogni glifo in `px_bounds()` con
`f32::floor()` / `f32::ceil()` (→ istruzioni wasm `f32.floor`/`f32.ceil`). Il
`.cwasm` è stato compilato col codegen **baseline SSE2** (il target cross
`x86_64-unknown-none` non abilitava SSE4.1), quindi cranelift NON inlinea
`ROUNDSS` ma emette una **libcall** (`FloorF32`/`CeilF32`/`TruncF32`/`NearestF32`).

Nel runtime **Wasmtime no_std** di ruos quelle libcall di arrotondamento float
**non vengono risolte correttamente** (ritornano l'input invece di floorare),
mentre nel runtime **std** del PC funzionano (`val.wasm_floor()` → `self.floor()`
hardware vs no_std → `libm::floorf`, ma il problema è la risoluzione della
libcall, non `libm` che è corretto). Risultato: `px_bounds` con valori
frazionari → dimensioni/coperture glifo sbagliate → atlante font corrotto →
testo garbled. Le forme (rettangoli, wallpaper) usano il texel bianco, non
floor/ceil per-glifo → nitide.

Isolata leggendo i sorgenti + un probe in `OutlinedGlyph::draw` (vendoring
diagnostico, poi rimosso): stesso glifo, stessa scala (`hf=0.0035855`), stesso
outline → su PC `bb=1.000,-6.000,8.000,0.000` (interi, floor applicato), su ruos
`bb=1.915,-5.489,7.404,0.000` (frazionari, floor NON applicato). `0` istruzioni
`ROUNDSS` nel cwasm. `libm::floorf/ceilf/truncf` verificate corrette nel kernel.

## Fix
`tools/wt-precompile`: forzato SSE4.1+ nel codegen cranelift
(`config.cranelift_flag_set("has_sse41"/"has_sse42"/"has_ssse3"/"has_sse3",
"true")`). Ora cranelift inlinea `ROUNDSS`/`ROUNDSD` per floor/ceil/trunc/nearest
→ eseguiti sulla CPU (corretti), bypassando del tutto la libcall no_std rotta.
Il cwasm passa da 0 a 35 `ROUNDSS`. Il runtime kernel resta compatibile: il suo
`detect_host_feature` riporta già `sse4.1` presente, quindi il `.cwasm` con
`has_sse41` deserializza senza modifiche al kernel. Verificato in QEMU+KVM: testo
nitido, identico al PC.

## Bonus fix (bug reale trovato durante l'indagine)
`kernel/src/wasm/wt/platform.rs` — `wasmtime_mmap_new` azzerava solo il ramo
`PROT_WRITE`, ma la linear memory wasm passa per `reserve(prot=0)` + `mprotect`,
quindi non veniva mai azzerata (violazione della garanzia di zero-init wasm su
cui si fida `alloc_zeroed`). Ora `mmap_new` azzera sempre (mappa writable →
`write_bytes(0)` → declassa ai flag richiesti). Boot-check di regressione
`platform::zero_init_self_test()`: FAIL pre-fix, ok post-fix
(`wt: linear-mem zero-init self-test ok`).

## File toccati
- tools/wt-precompile/src/main.rs (SSE4.1 in codegen — fix principale)
- kernel/src/wasm/wt/platform.rs (zero-init mmap_new + zero_init_self_test)
- kernel/src/boot/phases/interrupts.rs (wiring boot-check zero-init)
