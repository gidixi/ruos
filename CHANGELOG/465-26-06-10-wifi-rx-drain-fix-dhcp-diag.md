# 465 — WiFi RX drain: non fermarsi sui frame scartati + diagnostica tipo DHCP

**Data:** 2026-06-10

## Cosa

Due interventi sul datapath SP-WIFI-5 (debug "DHCP non si chiude"):

1. **Fix RX drain** — `recv_data` ritorna ora `RxPoll::{Frame,Rejected,Empty}`
   invece di `Option<usize>`: un frame consumato-ma-scartato (beacon, mgmt,
   chiave estranea, errore crc/ICV) non è più indistinguibile da "bulk-IN
   vuota". Il loop RX di `poll_io` fa `continue` su `Rejected` e `break` solo
   su `Empty`. Prima, un singolo beacon troncava il drain dell'intero poll
   (~1 frame/20 ms su aria affollata) → overflow della FIFO RX del chip →
   frame unicast persi (candidato principale per l'ACK DHCP mai visto).

2. **Diagnostica tipo messaggio DHCP** (capped, solo log) — helper
   `dhcp_msg_type` (parse opzione 53 BOOTP); log egress
   `dhcp tx type=…` (1=DISCOVER, 3=REQUEST) e campo `dhcp=` aggiunto alla
   riga ingress `ip ethlen=…`. Sul prossimo test HW discrimina le tre code
   possibili: OFFER rigettata da smoltcp (nessuna REQUEST, re-DISCOVER
   periodico) / REQUEST inviata ma ACK perso in aria-FIFO / ACK arrivato ma
   rigettato (es. subnet mask mancante — smoltcp la richiede per l'ACK).

Contesto (audit workflow su smoltcp 0.11.0 vendored + reference rtl8xxxu):

- RX decap **confermato corretto**: con RCR=0x7000680E il buffer è
  `[hdr 802.11][CCMP 8B][payload][MIC 8B]` senza FCS (APPEND_FCS BIT31 non
  settato; APPEND_ICV/MIC sì) — la matematica 376=24+8+8+328+8 torna, il
  frame Ethernet da 342 B per smoltcp è pulito.
- smoltcp 0.11 **accetta** OFFER IP-unicast con iface 0.0.0.0 (carve-out
  dhcpv4 prima del filtro dst-IP); accettata l'OFFER NON emette eventi:
  manda subito la REQUEST broadcast e `Configured` arriva solo con l'ACK
  (che richiede l'opzione subnet mask). Drop silenziosi possibili solo su
  chaddr/xid mismatch dentro `dhcpv4::process`.

## Perché

L'OFFER decifrata raggiunge smoltcp (parsed=1 et=0800) ma `dhcp bound` non
arriva mai. L'audit ha scagionato la ricostruzione del frame (ipotesi 1) e
ristretto il problema alla gamba REQUEST/ACK o ai gate interni di
`dhcpv4::process`; il fix del drain rimuove il difetto reale trovato e la
diagnostica rende il prossimo run HW conclusivo.

## File toccati

- kernel/src/usb/wifi/mod.rs
