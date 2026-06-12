# 479 — Installer SSD: ripara copy da modello bin.bgz

**Data:** 2026-06-12

## Cosa

Riparato `copy_boot_payload` (installer SSD), rotto dal passaggio al modello
archivio `bin.bgz`:

- **Data partition** ora scompatta OGNI membro di `bin.bgz` direttamente su
  `/bin/<name>` (parse RBIN + `pack::decompress_member`, skip-on-OOM sui membri
  giganti come `unpack_bin`). Prima iterava `modules::all()` cercando moduli loose
  `/bin/*.wasm` che NON esistono più → scriveva ZERO tool.
- **ESP**: `shell.wasm` (ora membro di `bin.bgz`, non più modulo loose) viene
  estratto dall'archivio e scritto su ESP `/bin/shell.wasm`, così la slim
  `limine-ssd.conf` lo module-carica al boot. `init.wasm`/`init.sh` restano loose.
- **`unpack_bin`**: gira prima di `storage` e panicava se mancavano archivio E
  set rescue (caso SSD slim). Ora: archivio assente + rescue assente = boot SSD
  installato atteso → WARN morbido, niente panic; i tool arrivano da `/mnt/bin`
  (montato dalla fase storage successiva). Il panic resta solo per archivio
  presente-ma-corrotto senza rescue.

## Perché

Timeline regressione:
- `cb451ef` (3 giu): installer nasce assumendo tool come moduli loose
  `/bin/*.wasm` in `limine.conf`.
- `fa0b772` (10 giu 11:59): tutti i `/bin/*.wasm` impacchettati in un solo
  `bin.bgz` (`/archive/`); moduli loose rimossi da `limine.conf`.
- `fb97768` (10 giu 12:03): installer "aggiustato" → aggiunge solo lo *skip* di
  `/archive/`, senza ripuntare la copia sull'archivio.

Risultato: SSD installato = ESP senza `shell.wasm` + `/mnt/bin` vuoto = sistema
morto. Ora l'SSD specchia il `/bin` live.

## File toccati
- kernel/src/disk.rs
- kernel/src/boot/phases/unpack_bin.rs
