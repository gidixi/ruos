# 430 — WiFi debug DHCP no-ACK: probe TIM/deauth/unicast + static IP su iface wifi

**Data:** 2026-06-11

## Cosa

Test HW post-fix-drain (CHANGELOG 426) ha ristretto il bug: DISCOVER→OFFER ok,
REQUEST inviata (tipo 3, retry smoltcp a +5s puntuale), **ACK mai ricevuto** —
e dopo l'OFFER **zero unicast RX** mentre il multicast GTK continua a fluire
(drain fix verificato funzionante: junk drenato a raffica). Sintomo compatibile
con AP che ci crede in power-save (bufferizza l'unicast) o STA droppata
silenziosamente. Tre probe diagnostiche read-only in `recv_data` (sopravvivono
al cap generale RX_DBG):

1. **TIM**: nei beacon del NOSTRO BSSID, walk IE→TIM (id 5) e test del bit del
   nostro AID → `tim: aid=N buffered=0/1` (log su transizione, cap 16). Bit a 1
   = l'AP tiene frame unicast bufferizzati per noi = ci crede in power-save.
2. **Mgmt**: deauth (c0) / disassoc (a0) / action (d0) indirizzati a noi o
   broadcast dal nostro AP → `mgmt to us: fc0=.. body=..` (reason code).
3. **Unicast data al nostro MAC**: cap dedicato → risponde a "arriva QUALSIASI
   unicast dopo l'OFFER?" (es. ARP reply).

Supporto: `WifiState.aid` (dall'assoc response, serve per il bit TIM) +
`ruos_net_set_static` ora applica l'IP statico a `iface_wifi` quando attached
(prima solo wired) → test killer: `wificonnect` poi `ifconfig` statico con
l'IP offerto e `ping <gw>`. Se l'ARP reply non torna = unicast RX morto a
livello link (conferma PS/drop); se il ping passa = datapath sano e il
problema è solo la consegna dell'ACK DHCP.

## Perché

Discriminare con un solo giro su HW reale le ipotesi rimaste: AP power-save
buffering / deauth silenziosa / ACK perso solo a livello DHCP. Il contenuto
della REQUEST è scagionato (stesso smoltcp 0.11 ha fatto bind su questa LAN
via RTL8169 wired; costruzione verificata sul sorgente vendored).

## File toccati

- kernel/src/usb/wifi/mod.rs
- kernel/src/wasm/host/proc.rs
