# 480 — Installer guidato con barra di avanzamento (stile Debian)

**Data:** 2026-06-12

## Cosa

Refactoring completo dell'installer ruos con:

**Kernel — session API stepped (`kernel/src/disk.rs`):**
- `InstallSession` — struttura che mantiene lo stato (porta AHCI, layout GPT,
  membri bin.bgz precalcolati) tra le chiamate; permette al tool WASM di
  riottenere il controllo dopo ogni file copiato.
- `session_open(port_idx, esp_mib)` — GPT+FAT, raccoglie i membri dell'archivio
  in un Vec indicizzato (senza decomprimere), ritorna la sessione.
- `session_step(&mut session)` — copia UN file (ESP o data partition, fresh
  FatWriter ogni volta), ritorna `(done_count, display_name)`. All'ultimo file
  esegue anche il flush su disco.
- Helper privati `esp_step_single` / `data_step_single` / `collect_members` /
  `find_module` (equivalenti ai path di `copy_boot_payload`, mantenuto per `mkboot`).

**Kernel — host fn (`kernel/src/wasm/host/proc.rs`):**
- `static INSTALL_SESSION: Mutex<Option<InstallSession>>` — al più una sessione
  per volta.
- `ruos::install_open(target, esp_mib) → total_files | error` — guard /mnt,
  acquisisci porta, apri sessione.
- `ruos::install_step(name_buf, name_cap) → done_count | error` — copia un file,
  scrive il nome in un buffer WASM (C-string). Chiude automaticamente la sessione
  a completamento o su errore.

**Tool WASM (`user/install/src/main.rs`):**
- **Modalità interattiva** (no argomenti): lista dischi, prompt target + ESP MiB
  con default, conferma esplicita "Type YES to confirm" (guard anti-errore),
  barra di avanzamento in-place via `\r` + blocchi Unicode `█`/`░`.
- **Modalità batch** (`install <n> [esp_mib]`): stampa `[ N/M] filename` per ogni
  file; emette `"install: ok -- remove the installer medium and reboot"` alla fine
  (compatibile con m2b2-test.sh e init script esistenti).

## Perché

Installer precedente bloccava il tool WASM per tutta la durata della copia (una
sola host fn `ruos_install`): nessuna informazione di progresso possibile. Aggiunta
la session API stepped per dare il controllo al tool tra ogni file, permettendo
display interattivo senza cambiare l'architettura kernel (no thread, no callback).

## File toccati
- kernel/src/disk.rs
- kernel/src/wasm/host/proc.rs
- user/install/src/main.rs
