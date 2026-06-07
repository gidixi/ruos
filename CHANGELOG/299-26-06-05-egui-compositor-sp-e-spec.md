# 299 — Spec SP-E: porta le app come finestre + ritira gui.cwasm

**Data:** 2026-06-05

## Cosa
Spec di SP-E (ultimo del Modello A): About/Files/Terminal/System-Monitor diventano
finestre del compositor; le voci del launcher (già elencate da SP-D) si accendono;
`gui.cwasm` esce dalla build/ISO di default (codice tenuto).

Decisioni (brainstorm):
- **Strategia A — 4 crate cdylib thin** (1 .cwasm per app: about/files/terminal/system
  su ruos-window, ognuno `frame()` → `frame_once(app.title(), |ctx| CentralPanel{ app.ui(ui) })`).
  Isolamento reale "un'app = un crate"; `wm.spawn(id)`→`/bin/<id>.cwasm`. +~36 MB disco
  (ISO ~115 MB); heap invariato (solo finestre aperte costano ~48 MB).
- **Tutte e 4 as-is** (UI vera, dati placeholder/simulati: Files/Terminal stub, System
  con CPU/mem/processi simulati). I **dati reali** (System legge proc::list/CPU/mem;
  Terminal con PTY reale) = **SP-F** futuro (host fn dati).
- **Ritiro gui.cwasm** dalla build/ISO di default + comando `gui`; `ruos-backend`/`Desktop`
  restano nel submodule (git history + ricostruibili), non shippati.

Parti: gui-core (verifica struct DeskApp già pub) → 4 crate-finestra → 4 regole
Makefile + ship + limine entry → ritiro gui.cwasm. Costruttori: AboutRuos / Files
(unit), Terminal::default(), System::default(); 560×420 (System 720×520).

Verifica: boot → desktop → "☰ Apps" → click ognuna → finestra egui; 2-3 aperte =
indipendenti; gui.cwasm assente; boot-check + screendump + VBox.

## Perché
Completa il Modello A: tutte le app del desktop sono finestre del compositor; il
compositor è la sola GUI (gui.cwasm ritirato).

## File toccati
- docs/superpowers/specs/2026-06-05-egui-compositor-sp-e-apps-as-windows-design.md
