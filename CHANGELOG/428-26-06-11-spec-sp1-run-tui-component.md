# 428 — Spec SP1 epoch watchdog: aggiunta policy run_tui_component

**Data:** 2026-06-11

## Cosa

Aggiornata `docs/superpowers/specs/2026-06-10-multi-tenant-hardening-design.md`
(SP1, epoch watchdog): il runner componenti TUI introdotto con CHANGELOG 427
(`kernel/src/wasm/wt/component.rs::run_tui_component`) è una superficie
Wasmtime in più senza deadline né kill cooperativo. Aggiunti:

- riga nella tabella "stato di fatto" (niente fuel, `kill <pid>` ignorato,
  runaway tiene l'AP, `poll_key` spin al 100%);
- punto 5 di policy: `epoch_deadline_callback` sullo store dell'app TUI con
  check `is_kill_pending` + VINTR (stesso meccanismo dei tool), più check
  kill e backoff dentro il loop di `poll_key`;
- caso di test: `kill` da seconda sessione SSH termina rtop con terminale
  ripristinato; variante runaway trappata entro ~100 ms.

## Perché

Lo split component-model di rtop (427) ha creato un path di esecuzione che
SP1 non copriva; il watchdog lì risolve anche i due difetti funzionali noti
del runner (kill ignorato, AP monopolizzabile).

## File toccati

- docs/superpowers/specs/2026-06-10-multi-tenant-hardening-design.md
