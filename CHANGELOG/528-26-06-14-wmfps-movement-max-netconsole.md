# 528 — wm-fps: PEAK/MAX + commit_mesh size telemetry per il lag del menu (misura, non fix)

**Data:** 2026-06-14

## Cosa

Strumentazione di MISURA (nessun fix) per localizzare il lag dell'interazione col
menu sul desktop full-screen. Tutto gated `#[cfg(feature = "wm-fps")]`, tutto FUORI
dal core raster bit-identico (`ruos-raster`/`compose.rs`): solo letture TSC nel loop,
contatori e `fetch_max` accanto agli accumulatori `RP_*` esistenti.

Aggiunto in `kernel/src/wasm/wt/wm.rs`:

- **PEAK per-iter (la causa: le AVG nascondevano lo spike)**: `iter_max`, `ra_max`,
  `pr_max` (companion MAX di `iter_sum`/`ra_sum`/`pr_sum`) e si ESPONE `fa_max_us`
  (era calcolato e buttato via con `let _ = fa_max_us;`).
- **Damage PEAK**: `RP_ROWS_MAX`, `RP_AREA_MAX` (w*h px), `RP_BANDS_MAX` catturati in
  `dispatch_raster` accanto a `RP_LAST.store` → intercetta lo spike di damage
  full-screen di un singolo frame (es. patch font-atlas → damage forzato full →
  fan-out SMP) invisibile nelle medie d/p/r/c.
- **Dimensione mesh in ingresso**: nel host fn `wm.commit_mesh`, `CM_CALLS` (commit
  per finestra nel report), `CM_BYTES_MAX`, `CM_VERTS_MAX`, `CM_IDX_MAX`,
  `CM_PRIMS_MAX` (wire: 20B/vert, 4B/idx, 32B/prim) → quantifica quanto è grande la
  mesh che la shell full-screen spedisce a ogni movimento ("shell full-screen vs
  finestra piccola").
- **Due nuove righe netconsole** al report 1 s: `wmfps3` (ITERmax/fa_max/ra_max/
  pr_max + dmg rows/area/bands max) e `wmfps4` (commit_mesh sizes).
- **4ª riga overlay on-screen** (`M it: fa: pr: ra:ms`) coi PEAK in ms, fallback
  se netconsole non emette su questo NIC (netconsole è "real-HW pending"). I valori
  overlay si aggiornano 1×/s e restano STABILI tra i report (non sfarfallano per
  frame), quindi leggibili anche durante il movimento.

Build di misura: `make iso CARGO_FEATURES="wm-fps netconsole"` + `tools/netconsole-rx`.

## Perché

I fix 513–527 sono stati fatti a tentativi misurando solo lo stato IDLE (mouse fermo,
loop in hlt). Lo stato laggy REALE — movimento continuo sul menu — non è MAI stato
misurato, e tutti i campi overlay sono AVG su ~1 s (tranne `b`=last), quindi lo spike
per-iter che si SENTE come lag era nascosto. Servono i PEAK e i conteggi durante il
movimento per localizzare la fase dominante (frame_all egui / present full-screen /
clone canvas / plan_damage) PRIMA di qualsiasi fix. L'utente ha ethernet → netconsole
porta il payload ricco che l'overlay non può contenere.

## File toccati

- kernel/src/wasm/wt/wm.rs
