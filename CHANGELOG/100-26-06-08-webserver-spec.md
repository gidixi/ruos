# 100 — Spec webserver HTTP (picoserve kernel-native)

**Data:** 2026-06-08

## Cosa
Aggiunta spec di feasibility per un servizio webserver HTTP in ruos, basato su
`picoserve` (HTTP server async no_std) compilato kernel-native, riusando il net
stack esistente (`net::sockets`, smoltcp) sullo stesso modello del server SSH.
Nessun codice scritto — solo documento di design. Documenta: analisi
compatibilità picoserve, adapter `embedded-io-async`/Timer, concorrenza
multi-core via `spawn_on`/`pick_compute_core`, caveat I/O (poll smoltcp + lock
NET su core 0), e la decisione architetturale aperta (server kernel-native A vs
app `.wasm` B vs `.cwasm` C).

## Perché
Valutare come aggiungere accesso HTTP a ruos. Spec scritta per essere ripresa a
freddo in una sessione successiva con tutto il contesto e le decisioni aperte.

## File toccati
- docs/superpowers/specs/2026-06-08-webserver-picoserve-design.md
- CHANGELOG/100-26-06-08-webserver-spec.md
