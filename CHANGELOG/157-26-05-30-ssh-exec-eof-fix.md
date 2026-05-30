# 157 — SSH Task 7: fix early-EOF truncation (exec + piped stdin)

**Data:** 2026-05-30
**Status:** DONE. `make run-ssh-test` passa exec + interattivo.

## Cosa

Risolto il troncamento dell'output sulle sessioni non-interattive
(`ssh host cmd`, stdin in pipe), diagnosticato in CHANGELOG 156.

### Causa

sunset 0.4.0 `Channel::handle_eof` rispecchiava un `CHANNEL_EOF` sul lato
output del server appena il client chiudeva stdin (early EOF). Il server
chiudeva la propria metà di uscita prima che la shell producesse output → il
client scartava i `CHANNEL_DATA` successivi.

### Fix (sunset vendored in `third_party/sunset/`)

- `channel.rs::Channel::handle_eof`: NON auto-invia più l'EOF del server.
  Ricevere l'EOF del peer marca solo `state = RecvEof` + sveglia i reader. Le
  due direzioni del canale sono indipendenti: l'EOF di input del client non
  deve chiudere l'output del server.
- `channel.rs::ChanList::eof_close`: costruisce i pacchetti `CHANNEL_EOF` +
  `CHANNEL_CLOSE` per un canale (una volta sola, via `sent_eof`/`sent_close`).
- `runner.rs::Runner::send_channel_close`: API pubblica che invia
  EOF+CLOSE via `traf_out` (il buffer accoda i pacchetti, 4 KiB).

### Bridge (`kernel/src/ssh/sunset_io.rs`)

- Rilevamento fine-shell: quando il processo rilascia il suo PTY
  (`pty::is_claimed(idx)==false`) e l'output residuo è drenato
  (`pty::master_output_len(idx)==0`), il bridge chiama
  `runner.send_channel_close(c)` e passa a `closing`.
- Loop termina quando `closing` e `output_buf` è vuoto; teardown fa un flush
  best-effort di `output_buf` sul socket prima di chiudere → EOF/CLOSE
  raggiungono il client.

### Supporto

- `kernel/src/pty/mod.rs`: `is_claimed(idx)`, `master_output_len(idx)`
  (peek non distruttivo).
- `kernel/src/executor/mod.rs` `ssh_pty_dispatcher_task`: rilascia il PTY
  anche sui path di errore di spawn (read/instantiate), così il bridge chiude.

## Verifica

`make run-ssh-test` (target → `tests/ssh-shell-test.sh`, ora copre exec +
interattivo):
- exec `ssh host pwd` (stdin chiuso): prima 15 B troncati → ora **207 B** con
  output `pwd` → `/`.
- interattivo `ssh -tt` (stdin aperto): **201 B**, nessuna regressione.

Limite residuo (non bloccante): l'exec gira attraverso la shell interattiva,
quindi l'output include prompt + echo (`ruos:/$ …`) oltre al risultato. Un
exec "pulito" (solo output del comando, senza prompt) richiederebbe una
modalità shell non-interattiva — polish oltre Task 7.

## File toccati

- third_party/sunset/src/channel.rs (handle_eof, ChanList::eof_close)
- third_party/sunset/src/runner.rs (Runner::send_channel_close)
- kernel/src/ssh/sunset_io.rs (close-on-exit + teardown flush)
- kernel/src/pty/mod.rs (is_claimed, master_output_len)
- kernel/src/executor/mod.rs (release pty su errore spawn)
- tests/ssh-shell-test.sh (aggiunto caso exec)
