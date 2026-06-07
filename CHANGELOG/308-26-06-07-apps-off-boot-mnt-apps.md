# 308 — App fuori dal boot: spedite su /mnt/apps, non più moduli Limine

**Data:** 2026-06-07

## Cosa

Le 5 app desktop (`about`, `files`, `terminal`, `system`, `notepad`, ~45 MB di
`.cwasm`) **non sono più moduli di boot Limine**. Prima erano dichiarate in
`limine.conf` → il bootloader le caricava **tutte in RAM** all'avvio, e
`modules.rs::mount_all()` le **ricopiava** nella tmpfs `/bin` (heap), anche se
nella sessione non le aprivi mai. Ora:

- `limine.conf`: rimosse le 5 coppie `module_path`/`module_cmdline` delle app
  (restano solo `shell.cwasm` + `compositor.cwasm` come moduli di boot).
- `Makefile`: il target `iso:`/`test-boot:` non copia più gli app `.cwasm` in
  `$(ISO_ROOT)/bin`. Nuovo target **`apps-on-disk`** che **ricrea un'immagine FAT32
  pristina** `build/disk.img` e ci stage le app sotto `::/apps` (→ `/mnt/apps`).
  Si ricrea da zero (anziché appendere) perché `mtools mmd` si impianta su una
  directory già esistente e su un FAT "sporco" lasciato da QEMU; partire da un
  `mkfs` fresco rende `make run` deterministico. `run:` ora dipende da `apps-on-disk`
  (nota: rigenera il disco → ri-esegui `make ssh-key-on-disk` se ti serve la pubkey).
- A runtime il **launcher dinamico** (CHANGELOG 307) scopre le app in `/mnt/apps`
  via `scan_apps`, e `wm.spawn`/`module_by_name` le carica on-demand dal disco
  (cerca `/bin` poi `/mnt/apps`).

Effetto: la ISO scende da **60016 → 37921 settori** (~45 MB in meno) e il
bootloader non carica più ~45 MB di app in RAM ad ogni avvio; un'app viene letta
dal disco solo quando la lanci.

## Perché

Il bootloader caricava eagermente *tutti* i bin (e il kernel li ri-copiava in
heap), sprecando RAM e tempo di boot per app spesso inutilizzate. Spostandole su
un filesystem letto on-demand (FAT32, driver già esistente) si allinea al modello
"app in una cartella, kernel le pesca da lì". Per aggiungere un'app non serve più
toccare `limine.conf`/boot: basta il `.cwasm` in `/mnt/apps`.

Nota: la live-CD senza disco non ha le app (solo shell+desktop); il sistema con
disco/SSD sì. La soluzione "corretta" per live-CD (driver ISO9660+ATAPI così la
ISO stessa è letta on-demand) resta una direzione futura. I ~3.2 MB di coreutils
`.wasm` sono ancora moduli di boot (spostarli su `/mnt/bin` è un follow-up facile,
il PATH della shell già li cerca lì).

## File toccati

- limine.conf (rimosse le 5 app dai moduli di boot)
- Makefile (`iso:`/`test-boot:` senza copia app in /bin; nuovo `apps-on-disk`;
  `run:` dipende da `apps-on-disk`; var `APP_CWASMS`)
