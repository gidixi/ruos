# 485 — WM: APP_DIRS include /mnt/bin (SSD installato trova shell.cwasm)

**Data:** 2026-06-12

## Cosa

Aggiunto `/mnt/bin` a `APP_DIRS` in `kernel/src/wasm/wt/wm.rs`.

Prima: `APP_DIRS = ["/bin", "/mnt/apps"]`  
Dopo:  `APP_DIRS = ["/bin", "/mnt/bin", "/mnt/apps"]`

Aggiornato il guard `is_mounted` da `*dir == "/mnt/apps"` a `dir.starts_with("/mnt")`
in entrambi i punti che iterano `APP_DIRS`: `read_app_bytes` e `scan_apps`.

## Perché

Su SSD installato, `shell.cwasm` (desktop GUI) e tutti i `.cwasm` delle app
stanno in `/mnt/bin/` (data partition), non in `/bin/` (che sull'ESP slim ha
solo `shell.wasm` CLI). Il compositor cercava `shell.cwasm` solo in `/bin` e
`/mnt/apps`, non lo trovava, e ricadeva sull'`egui-demo` embedded nel kernel
(finestra demo ciclocromatica). Stesso problema per il launcher: `scan_apps`
non scansionava `/mnt/bin` e non mostrava nessuna app.

Con `/mnt/bin` in `APP_DIRS`:
- Boot da SSD: compositor trova `/mnt/bin/shell.cwasm` → carica il desktop corretto.
- Launcher: `scan_apps` trova tutti i `.cwasm` in `/mnt/bin` → li mostra nel pannello.
- Live ISO: `/bin` ha già `shell.cwasm` in tmpfs → vince per priorità, `/mnt/bin`
  skippato (se /mnt non montata) o ignorato (stessa copia).

## File toccati
- kernel/src/wasm/wt/wm.rs
