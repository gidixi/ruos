# 51 — Review fixes: FB console (Step 8 Task 4)

**Data:** 2026-05-28

## Cosa

Applicate le tre Important del code-quality review su `console/fb.rs`:

- **I1 — atomics morti rimossi**
  - `FB_PIXEL_BGR`: scritto in `FramebufferConsole::new` ma mai letto. Eliminato.
  - `CURSOR_SHOWN`: XOR'd in `tick_cursor` ma mai letto. Eliminato.
  - Eliminata la shim `_force_use` che mascherava i due dead-code warning.
  - `AtomicBool` rimosso dagli `use`.

- **I2 — clamp difensivo cursor**
  - In `csi_dispatch` rami `B`/`C`/`H`: `self.rows - 1` / `self.cols - 1`
    → `self.rows.saturating_sub(1)` / `self.cols.saturating_sub(1)`.
  - Evita underflow su FB degenere a 0 righe/colonne.

- **I3 — no-alloc SGR**
  - Rimosso il `collect::<Vec<u16>>()` nel ramo `'m'`. `apply_sgr` accetta
    già `impl Iterator<Item = u16>`: gli si passa direttamente
    `params.iter().flat_map(|p| p.iter().copied())`.

Verifica: `make build` OK (warning preesistenti, nessuno nuovo),
`make run-test` PASS (`ruos: ticks=204`). Tutti i log Step 8 invariati:
`fb ok / fb test ok / fb attached / [31mERR[0m hello via ansi / ansi test ok`.

## Perché

Snellire l'implementazione del FB console subito dopo il merge dei 4 task,
prima di chiudere lo Step 8 e mergiare `feature/fb-console` in `main`.
Gli atomics morti erano debito da Task 1, le clamp negative una landmine
per future fb-resolution edge case, e l'allocazione SGR un cost-per-byte
inutile sul fast path del print.

## File toccati

- kernel/src/console/fb.rs (−17 +9)
- CHANGELOG/51-26-05-28-fb-review-fixes.md (nuovo)
