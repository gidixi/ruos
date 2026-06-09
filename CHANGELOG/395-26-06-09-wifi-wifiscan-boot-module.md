# 395 — WiFi: `wifiscan` nel set boot-module minimo (limine.conf)

**Data:** 2026-06-09

## Cosa
Aggiunto `wifiscan.wasm` al "Live-CD fallback set" di `limine.conf` (i boot module
caricati direttamente nel tmpfs `/bin` iniziale), accanto agli altri tool
diagnostici di rete (ip/ifconfig/ping). Senza, `wifiscan` esisteva nell'ISO `/bin`
ma non era nella **bundle minima** disponibile diskless / prima dell'overlay `/bin`
completo — quindi non lanciabile al boot su HW reale finché non montava il `/bin`
esteso.

## Perché
`wifiscan` è un tool di debug HW: deve essere disponibile subito al boot, come
lsusb/ip/ifconfig, anche diskless.

## File toccati
- limine.conf
