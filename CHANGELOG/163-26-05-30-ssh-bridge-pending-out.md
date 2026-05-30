# 163 — SSH bridge: buffer locale per output drop sotto back-pressure

**Data:** 2026-05-30

## Cosa
`kernel/src/ssh/sunset_io.rs`: il bridge PTY → SSH channel ora bufferizza
in un `pending_out: Vec<u8>` i byte che `runner.write_channel` non ha
accettato in un iteration, e li ritrasmette al passaggio successivo.
In precedenza venivano **scartati silenziosamente** (vedi il vecchio
commento `// Bytes lost — TODO: buffer locally`) e finivano sulla
metrica `tx_dropped` del log di sessione.

La condizione di chiusura del canale ora attende anche `pending_out.is_empty()`,
così l'output finale arriva al client invece di essere troncato.

## Perché — il bug riportato dall'utente
"Scrivo i comandi da client remoto verso il server ma non ricevo come
output niente — se vado sulla schermata di VirtualBox vedo che i
comandi creano output su quella schermata."

I test interni (`make run-ssh-test`, `run-passwd-test`,
`run-passwd-diskless-test`) tutti passavano: in QEMU con SLIRP la rete
è virtuale + bassa latenza, `write_channel` accetta sempre il buffer
intero. Su VirtualBox **bridged** (LAN reale, RTT variabile,
TCP windowing diverso) il sunset Runner saturava la window di canale
a brevi raffiche, write_channel restituiva `w < filled` o `Err`, e i
byte rimanenti venivano persi. Lo shell sulla PTY 1 funzionava
correttamente — l'output del comando era prodotto, finiva nel
`master_out[1]`, veniva letto dal bridge, ma poi cadeva nel buco.

Il sintomo "lo vedo sulla schermata VBox" si spiega col fatto che dopo
un drop l'utente vedeva lo shell di **boot** sul framebuffer
continuare il suo loop di prompt-redraw (è quello che pensava fosse
"output dei suoi comandi" — non lo era; era solo lo shell locale
non-SSH che disegna il suo prompt).

## Note
- `tx_dropped` rimane nel log come campo (sempre 0 ora). Tenuto per
  compat dei consumatori di log; futuro cleanup.
- Mantengo il limite di lettura di 64 byte/iter dalla PTY: con il
  buffer locale anche raffiche maggiori vengono assorbite e
  trasmesse senza perdita.

## Test
- ✅ `make run-ssh-test` → TEST_PASS_SSH
- ✅ `make run-passwd-test` → TEST_PASS_PASSWD
- ✅ `make run-passwd-diskless-test` → TEST_PASS_PASSWD_DISKLESS
  (regression locale invariato; il fix avrà effetto visibile solo
  in setup con back-pressure reale come VBox bridged.)

## File toccati
- kernel/src/ssh/sunset_io.rs
