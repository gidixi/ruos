# 162 — SSH avvia senza disco con password di default baked-in

**Data:** 2026-05-30

## Cosa
ruos ora boota in QEMU/VirtualBox **senza alcun disco SATA attaccato**
e l'SSH password funziona out-of-the-box con la password di default
`ruos`. Niente più step manuali (`make disk`, conversione VDI, attach
in VBox) prima del primo SSH.

- **`kernel/src/ssh/hostkey.rs`**: se la scrittura su `/mnt/host.key`
  fallisce (no /mnt), il server prosegue comunque con una host key
  generata ed effimera in RAM. Logga un warning: il fingerprint
  cambierà a ogni boot (i client vedranno il prompt "host key
  fingerprint" al primo connect, accettabile per uso demo). Prima il
  fallimento di scrittura abortiva la spawn dell'intero server SSH.
- **`kernel/src/ssh/password.rs`**: nuova enum `PasswordCheck` con due
  varianti `Pbkdf2` (da `/mnt/passwd`) e `Plaintext` (compile-time
  default). Se la lettura del file fallisce o l'hash non parsa, il
  server **usa il default compile-time** invece di disabilitare
  password auth. Confronto sempre constant-time.
- **Default password compile-time**: `option_env!("RUOS_DEFAULT_PASSWORD")`
  con fallback `"ruos"`. Override al build time:
  `make build RUOS_PASSWORD=hunter2`.
- **`kernel/build.rs`**: aggiunto `cargo:rerun-if-env-changed=RUOS_DEFAULT_PASSWORD`
  così cambiare la password di default invalida correttamente il cache.
- **`kernel/src/ssh/server.rs`**, **`sunset_io.rs`**: i tipi
  `PasswordHash` → `PasswordCheck` propagati attraverso il session ctx.
- **`Makefile`**: ricetta `build` ora passa `RUOS_DEFAULT_PASSWORD` a
  cargo. Aggiunto target `run-passwd-diskless-test`.
- **`tests/ssh-passwd-diskless-test.sh`** (nuovo): boota QEMU **senza
  `-drive`** e verifica che SSH password=`ruos` funzioni contro il
  fallback compile-time, e che il serial log mostri il messaggio
  `password fallback to built-in default`.

## Perché
La domanda dell'utente: "perché devo creare disk, collegare, generare
immagini? Non può avviarsi direttamente?". Sì, è la UX corretta per un
demo OS. Le precondizioni di /mnt erano un effetto collaterale del
design Step 16 (file persistenti per host key/authkeys/passwd); per
casi d'uso interattivi non servono. Il password baked-in è
volutamente meno sicuro (presente in chiaro nel binario kernel,
estraibile da chiunque abbia l'ISO) ma è esplicitamente etichettato
come "demo convenience, not a security mechanism" nel commento di
`password.rs`. Per uso "serio" si setta `RUOS_DEFAULT_PASSWORD` al
build, o si fornisce `/mnt/passwd` con un PBKDF2 vero che ha
precedenza sul fallback.

## Test
- ✅ `make run-test` → TEST_PASS (smoke completo, regression)
- ✅ `make run-ssh-test` → TEST_PASS_SSH (pubkey regression)
- ✅ `make run-passwd-test` → TEST_PASS_PASSWD (password da /mnt/passwd)
- ✅ `make run-passwd-diskless-test` → TEST_PASS_PASSWD_DISKLESS (nuovo)

## File toccati
- kernel/build.rs
- kernel/src/ssh/hostkey.rs
- kernel/src/ssh/password.rs
- kernel/src/ssh/server.rs
- kernel/src/ssh/sunset_io.rs
- Makefile
- tests/ssh-passwd-diskless-test.sh
