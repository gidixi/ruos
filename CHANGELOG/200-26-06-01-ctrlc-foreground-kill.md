# 200 — Ctrl-C chiude le app (VINTR → kill foreground)

**Data:** 2026-06-01

## Cosa
`Ctrl-C` ora termina l'app in foreground su un terminale (locale o SSH) e
riporta al prompt della shell. Prima non funzionava per nessuna app.

Due cause, entrambe risolte:

1. **La line discipline non uccideva niente.** Il ramo VINTR (0x03) in modo
   cooked si limitava a svuotare il line-buffer ed echeggiare `^C`. Ora la PtyPair
   traccia `foreground_pid` (impostato dall'exec worker mentre un figlio gira) e
   VINTR richiede il kill cooperativo di quel pid. Il kill è fatto DOPO aver
   rilasciato il lock della pair (`request_kill` prende il lock REGISTRY: ordini
   disgiunti = niente deadlock). Le letture dello slave (`vfs::devices` +
   `pty::slave_read_one_timeout`) ritornano EOF quando il foreground ha un kill
   pendente, così un'app bloccata su stdin si sblocca e l'app esce (il check kill
   in `Fiber::run` scatta → exit 137).

2. **La shell lasciava il terminale in raw.** `shell.wasm` fa `save_and_raw()`
   una volta (per il suo line editor) e NON ripristinava cooked prima di
   eseguire un comando → i figli ereditavano raw → VINTR non scattava mai.
   Fix lato kernel (vale per qualsiasi shell/contesto): l'exec worker fa lo
   snapshot del termios del chiamante, forza cooked (input canonico + segnali),
   esegue il figlio, poi ripristina. Le app che vogliono raw (rtop, nano) lo
   impostano da sé e il loro guard ripristina cooked prima del restore della
   shell.

## Perché
Requisito base di usabilità del terminale: poter interrompere un'app che gira.
Emerso testando rtop. ruos non ha segnali POSIX — questo è l'equivalente
cooperativo di SIGINT (kill flag controllato ai punti di sospensione host-fn).

## Limiti noti
- App puramente compute-bound senza sospensioni non muoiono istantaneamente al
  `^C` (il flag kill è controllato alla prossima host-fn); il limite di fuel le
  termina comunque. Le app I/O-bound (ping, cat, REPL) muoiono subito.
- Le pipeline (`cmd1 | cmd2`) non impostano ancora foreground_pid: `^C` su una
  pipeline non killa gli stage (solo `exec` singolo). Follow-up.

## File toccati
- kernel/src/pty/pair.rs (campo foreground_pid)
- kernel/src/pty/ldisc.rs (VINTR ritorna il pid + sveglia lo slave reader)
- kernel/src/pty/mod.rs (master_input_push kill, set_foreground, termios snapshot/set/force_cooked)
- kernel/src/vfs/devices.rs (EOF su kill foreground)
- kernel/src/executor/mod.rs (exec worker: snapshot+force_cooked+foreground+restore)
- tests/ctrlc-ssh-test.sh + Makefile run-ctrlc-test
