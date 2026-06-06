# 304 — Step 1 riformulato: fast cpu_id (RDTSCP) prima dell'allocatore per-core

**Data:** 2026-06-05

## Cosa
Aggiornata la spec macro
`docs/superpowers/specs/2026-06-05-smp-shared-nothing-migration-design.md` per
riformulare **Step 1** alla luce del risultato dello spike allocatore:

- **§0 TL;DR** — aggiunto punto 4: Step 1 riformulato dallo spike.
- **§4 Ordine di build** — Step 1 diviso in **1a (fast cpu_id: RDTSCP+TSC_AUX)** →
  **1b (allocatore per-core, ri-misurato)**.
- **§6** — riscritto Step 1: box "Risultato spike" (a `-smp 4`, default talc 19.7 ms
  vince; magazine +62%, per-core talc +92%, perché la tassa `cpu_id()` ~200 ns/alloc
  supera la contesa risparmiata). Nuovo **Step 1a**: `cpu_id()` veloce via `RDTSCP` +
  `IA32_TSC_AUX` (detect via CPUID + fallback LAPIC, diverse-system-safe), con la
  tabella di **verifica diretta su 3 ambienti** (QEMU/VirtualBox/HW i7-8700: RDTSCP+
  TSC_AUX ovunque; RDPID NON portabile). Step 1b (allocatore) declassato a "dopo 1a +
  ri-misura; se non vince, rinvia a Step 3".
- **§14** — il trade-off "costo cpu_id sul path caldo" marcato MISURATO e RISOLTO
  (via 1a; gs-base scartato perché rotto su VBox).

Correzioni di referenza: rimosse le citazioni errate a un inesistente "gap §13.6"
(il costo cpu_id viveva in §6/§14, non tra i 6 gap del critico).

**Aggiornamento risultato verificato (dopo build+verifica di 1a).** Implementato e
verificato il fast cpu_id (codice in `CHANGELOG/305`): `cpuid` da ~200 ns → 23 ns,
`make test-boot` PASS. Ri-misurato lo spike a `-smp 4` CON cpu_id veloce: default talc
14.9 ms, **magazine A 9.6 ms (−36%, vince)**, per-core talc B 12.2 ms (−18%). → spec
§6 Step 1b chiuso su **magazine A** (era "da ri-misurare"); §0.4 e §6.1b aggiornati col
dato; decision record `2026-06-05-allocator-architecture.md` portato da PENDING a
RESOLVED (§8): Option 1+2 = fast cpu_id (RDTSCP) → magazine A.

## Perché
Lo spike (piano `2026-06-05-smp-step1-allocator-spike.md`, decisione
`2026-06-05-allocator-architecture.md`) ha **provato con i dati** che un allocatore
per-core regredisce finché `cpu_id()` legge LAPIC MMIO (~200 ns) su ogni alloc. La
leva reale di Step 1 non sono le arene per-core ma un `cpu_id()` economico —
verificato fattibile e portabile via RDTSCP+TSC_AUX (incluso VirtualBox, dove gs-base
è rotto). La spec deve riflettere la direzione corretta prima di implementare.

## File toccati
- docs/superpowers/specs/2026-06-05-smp-shared-nothing-migration-design.md
- docs/superpowers/decisions/2026-06-05-allocator-architecture.md
- CHANGELOG/304-26-06-05-smp-step1-reformulated-rdtscp.md
