# 267 — Desktop egui: clock egui monotonico (fix flicker) + orologio idle + blit veloce

**Data:** 2026-06-04

## Cosa
Tre fix alla reattività/correttezza del desktop egui:

1. **`gfx_wall_secs` ora monotonico** (fix del flicker "le finestre si chiudono e
   riaprono" + stutter).
2. **L'orologio avanza anche da fermo** (prima si aggiornava solo muovendo il
   mouse).
3. **Blit framebuffer molto più veloce** (riduce il costo per-frame lato host).

## Perché / Root cause

### 1. Flicker animazioni (finestre che si chiudono/riaprono)
`gfx_wall_secs()` calcolava `rtc.second + (timer::ticks() % 100)/100`, mescolando
**due clock NON sincronizzati**: il secondo viene dalla CMOS RTC, la frazione dal
contatore tick LAPIC. Le loro fasi sono diverse, quindi la frazione tornava a
0.00 *prima* che il secondo RTC scattasse → il valore **saltava indietro di ~1s
ogni secondo**. egui usa `RawInput.time` come clock delle animazioni e **richiede
che sia monotonico**: con il tempo che indietreggiava, le animazioni di apertura
di finestre/menu andavano avanti e indietro (sembravano chiudersi e riaprirsi) e
tutto faceva scatti. Visibile soprattutto muovendo il mouse perché è l'unico
momento in cui lo schermo si ri-renderizza (vedi #2).

Confronto col PC (dove non flickera): il backend PC usa `GetLocalTime` con i
millisecondi — **un solo clock coerente** → sub-secondo monotonico. Il bug era
solo nel port ruos che combinava due sorgenti.

**Fix:** latch dell'offset wall UNA volta dalla RTC, poi avanzamento puro col
tick di uptime monotonico (risoluzione 10 ms). Non torna mai indietro
(egui-safe); l'orologio HH:MM resta corretto (wrap via `rem_euclid`); il drift
tick-vs-RTC in una sessione è sub-secondo, irrilevante per i minuti.

### 2. Orologio fermo / "si aggiorna solo se muovo il mouse"
Repaint on-demand: senza input nulla risvegliava il loop, quindi l'orologio
HH:MM restava fermo finché non si muoveva il mouse. **Fix:** il loop del backend
traccia il minuto wall e forza un singolo repaint quando cambia (chiamata host a
buon mercato: dopo il fix #1, `gfx_wall_secs` è una lettura atomica, niente I/O
RTC per-poll). Le animazioni egui già completavano da sole via
`has_requested_repaint()`.

### 3. Rendering lento / a scatti
Il `blit()` del kernel era un **loop per-pixel con bounds-check per-pixel su ~1M
px a ogni frame** (egui blitta tutto lo schermo ogni frame). **Fix:** clip della
sorgente UNA volta, poi fast-path 32-bpp: riga intera contigua su entrambi i lati
→ un `memcpy` per riga su framebuffer RGB; swap R/B stretto per riga su BGR;
nessun branch di clip per-pixel. Slow-path per-pixel mantenuta solo per fb
non-32-bpp. (NB: il costo dominante resta il raster software full-scene in
gui-core; vera fluidità richiede rendering a dirty-rect — follow-up separato.)

## Verifica
- Kernel ricompila pulito (solo warning dead-code preesistenti).
- `gui.cwasm` ricompilato (ruos-backend) + AOT-precompilato senza errori.
- `make test-boot` → TEST_BOOT_PASS, self-test zero-init ancora ok.

## File toccati
- kernel/src/wasm/wt/gfx.rs (wall_secs monotonico latchato)
- kernel/src/gfx/mod.rs (blit fast-path)
- ruos-desktop/ruos-backend/src/main.rs (repaint sul cambio minuto)
