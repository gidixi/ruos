# 511 — wm-fps: strumentazione ITER + HLT (diagnosi perf)

**Data:** 2026-06-13

## Cosa
Aggiunto al loop del compositor (gated `wm-fps`) il cronometraggio del **wall-clock
per iterazione** (`ITER`) e del **tempo speso in `hlt`** (`HLT`), riportati nella riga
`wmfps` e sulla riga 1 dell'overlay (`Nfps NHz iter:Xms hlt:Yms`). Serve a localizzare
una regressione perf su HW reale dove il loop gira a ~7Hz (143ms/iter) ma le 3 fasi
misurate (`frame_all`/`raster`/`present`) sommano ~13ms → ~130ms non spiegati.

`ITER` vs `(frame_all+raster+present+HLT)` dice se il tempo è: idle in `hlt` (loop
wake-starved → la GUI-core non viene svegliata, sospetto timer periodico assente
sull'AP-GUI) oppure lavoro non cronometrato. Verifica TCG idle: ITER=10ms ≈
frame_all 2.6 + raster 1.3 + HLT 5.6 → i conti tornano, niente costo nascosto a 100Hz.

## Perché
Il fix precedente (510, soglia inline dispatch_raster) NON ha cambiato i numeri su HW
→ il collo non è il fan-out raster. Strumento invece di indovinare (systematic
debugging): ITER/HLT localizza idle-bound vs work-bound prima del prossimo fix.

## File toccati
- kernel/src/wasm/wt/wm.rs (run loop: accumulatori hlt_sum/iter_sum/prev_iter; report
  wmfps + overlay riga 1)
