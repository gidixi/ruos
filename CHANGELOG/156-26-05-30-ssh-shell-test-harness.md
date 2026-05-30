# 156 — SSH interactive shell verified + test harness fix

**Data:** 2026-05-30
**Status:** Interactive SSH shell WORKING end-to-end. `make run-ssh-test`
passes (TEST_PASS_SSH).

## Cosa

### Diagnosi (Tasks 6-8 chiusi)

Verificato in QEMU che il server SSH funziona end-to-end: KEX, pubkey auth
ed25519 con firma reale, canale + PTY shell interattiva. Un client OpenSSH
con stdin tenuto aperto riceve il prompt `ruos:/$`, esegue `pwd` → `/`, e
`exit`. L'"hang di `progress()`" documentato in CHANGELOG 155 era già stato
risolto dai commit successivi (release profile + pending_in buffer).

Il sintomo "output della shell perso" osservato con `printf … | ssh` NON è un
bug del kernel/SSH/smoltcp. Catturato il traffico guest (QEMU `filter-dump`):
il guest trasmette tutti i ~1400 byte e il client li ACK-a a livello TCP, ma
OpenSSH ne emette solo i primi 15 su stdout. Causa (da `ssh -vvv`): il client
con stdin in pipe chiude subito stdin → invia `CHANNEL_EOF`; sunset 0.4.0
`Channel::handle_eof` (third_party/sunset/src/channel.rs:969) rispecchia
immediatamente un `CHANNEL_EOF` sul lato output del server, *prima* che la
shell produca output → il client chiude la metà di lettura e scarta i
`CHANNEL_DATA` successivi. Con stdin tenuto aperto (sessione interattiva
reale) il problema non si presenta. Documentato in
`kernel/src/ssh/sunset_io.rs` (commento sull'arm `SessionExec`).

**Limite residuo:** uso non-interattivo (`ssh host cmd`, stdin in pipe) viene
troncato finché `handle_eof` di sunset (ora vendored in `third_party/sunset/`)
non viene patchato per ritardare l'EOF del server alla chiusura effettiva
dell'output.

### Test harness

- Nuovo script committato `tests/ssh-shell-test.sh`: boot headless QEMU +
  virtio-net hostfwd, client `ssh -tt` con stdin tenuto aperto, esegue `pwd`
  + `exit`. PASS = serial contiene `auth ok` E il client riceve il prompt
  `ruos:/$`. Pulisce QEMU residui + attende la porta 2222 libera.
- `Makefile` `run-ssh-test`: il vecchio recipe inline si auto-uccideva — il
  suo `pkill -f 'qemu-system-x86_64.*hostfwd'` faceva match con la riga di
  comando del recipe stesso (`bash -c '…qemu-system-x86_64…hostfwd…'`),
  mandandosi SIGTERM ("Terminated"). Ora il target chiama lo script
  committato (la cui cmdline è `bash tests/…`, non matcha il pattern).

## Perché

Chiudere Tasks 6-8 con una verifica riproducibile e automatizzata, e lasciare
tracciato il limite non-interattivo con il punto preciso da patchare.

## File toccati

- tests/ssh-shell-test.sh (nuovo)
- Makefile (run-ssh-test → chiama lo script)
- kernel/src/ssh/sunset_io.rs (commento limite early-EOF — già in HEAD)
