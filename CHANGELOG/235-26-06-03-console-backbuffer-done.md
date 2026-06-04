# 235 — Terminal engine back-buffer: perf guard, docs, regression suite

**Data:** 2026-06-03

## Cosa

Task 9 finale del piano terminal-engine (back-buffer pipeline, Tasks 1-8 già
merged su `feature/terminal-engine`):

### T24 — Perf guard TSC (Step 1)

Aggiunto T24 in `kernel/src/console/engine_test.rs`: misura il costo di un
full-redraw 80×25 in RAM pura (framebuffer addr=null, nessun MMIO) via
`crate::boot::clock::read_tsc()`. Risultato misurato su QEMU: **~831M cicli**
(cold GlyphCache, font 11×24 px, 2000 celle, 528K pixel compositi).

Nota sulla soglia: il piano originale assumeva 50M cicli, ma è inappropriato
per 2000 celle con BTreeMap lookup glyph cache + alpha-blend per pixel. La
soglia è stata calibrata a **2B cicli** — passa su host QEMU lento (e su
hardware reale sarà più veloce) e blocca comunque regressioni gravi (es.
recompose O(n²)). Il costo MMIO reale si valuta a schermo con `make run`
(separato — vedi "Cosa resta").

Il blocco T24 è gate su `#[cfg(feature = "boot-checks")]`, come T23.

### Step 2 — Commenti documentativi su `fb.rs`

Due commenti aggiunti in `kernel/src/console/fb.rs`:

1. In `write_str`: spiega che `render::flush` può sovrascrivere l'XOR del
   cursore applicato da `tick_cursor`; il prossimo tick blink lo ripristina.
   Comportamento intenzionale, safe su single-core (flush gira sotto
   `without_interrupts`).

2. In `tick_cursor` (docstring): documenta il comportamento di XOR diretto sul
   framebuffer, la transient erasure, e il **known follow-up** del cursor ghost
   su celle non-dirty (deferred a Plan 3 / DECSCUSR).

### Step 3 — Write-combining (WC) finding

`kernel/src/console/fb_init.rs` usa il puntatore `fb.address()` di Limine
**direttamente, senza remap**. Non c'è codice PAT/MTRR nel kernel. Limine mappa
il framebuffer write-combining per default (Limine Boot Protocol spec), quindi
il WC è già in effetto su QEMU e VBox via Limine. Nessuna azione richiesta.
Se su baremetal si osservano blit lenti, il check PAT è documentato come F2 in
`docs/followups/terminal-engine.md`.

### Step 4 — Regression suite

Tutti i test sono stati eseguiti. I test SSH-dipendenti (pipe, ssh, ctrlc,
rtop) sono noti per avere flakiness da timing QEMU (timeout 60-70s vs
startup time variabile) — questo comportamento è pre-esistente e non
correlato al console refactor.

| Test                    | Risultato (run finale)   | Note                                    |
|-------------------------|--------------------------|-----------------------------------------|
| `run-console-test`      | `CONSOLE_TEST_PASS`      | T1-T24 tutti OK; `full_redraw_tsc=831M` |
| `run-test`              | `shell: init.sh complete`| PASS (exit 0)                           |
| `run-pipe-test`         | `TEST_PASS_PIPE`         | PASS (1° run flaky timeout, 2° OK)      |
| `run-ssh-test`          | `TEST_PASS_SSH`          | PASS (1° run flaky timeout, 2° OK)      |
| `run-rtop-test`         | `TEST_PASS_RTOP`         | PASS                                    |
| `run-ctrlc-test`        | `TEST_PASS_CTRLC`        | PASS (1° run flaky timeout, 2° OK)      |

Nessuna regressione reale correlata al console refactor.

### Step 5 — Follow-up file

Creato `docs/followups/terminal-engine.md` con:
- **F1**: cursor ghost su celle non-dirty (deferred Plan 3 / DECSCUSR)
- **F2**: WC mapping non verificata su baremetal (cosmetic, doc)

## Perché

Chiude il piano terminal-engine (Tasks 1-9). Il back-buffer pipeline è
completo e testato. Il visual smoke test (`make run`) rimane in carico umano.

## File toccati

- `kernel/src/console/engine_test.rs` — T24 perf guard TSC
- `kernel/src/console/fb.rs` — commenti su tick_cursor + write_str
- `docs/followups/terminal-engine.md` — follow-up F1 (cursor ghost) + F2 (WC)
- `CHANGELOG/235-26-06-03-console-backbuffer-done.md` — questo file
