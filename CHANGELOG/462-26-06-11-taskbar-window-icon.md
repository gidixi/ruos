# 462 — Icona app per le entry taskbar delle app in esecuzione

**Data:** 2026-06-11

## Cosa

Le entry della taskbar (app/finestre in esecuzione) nel pannello shell ora
mostrano l'icona `windows-svgrepo-com.svg` (finestre a cascata, a colori) al posto
del pallino glifo `●`/`○`, che on-device usciva come quadratino tofu (font senza
quel codepoint).

- `assets/icons/windows-svgrepo-com.svg`: ridotto a 48px (era 800px); PNG
  rigenerato con `xtask`.
- `gui-core/images.rs`: chiave `WINDOW_ICON`.
- `gui-core/desktop/shell.rs`: ogni entry taskbar = icona app (16px, a colori →
  niente tint; attenuata con alpha ridotta se la finestra è minimizzata) + titolo
  `selectable_label` cliccabile (focus = evidenziata). Fallback senza texture
  (anteprima PC): solo il titolo (niente glifo dot → niente tofu).
- `apps/shell`: embedda + decodifica + ri-stash `windows-svgrepo-com.png` come per
  l'icona menu.

## Perché

Richiesta utente: per le app in esecuzione usare l'SVG windows; il pallino
mostrava un quadratino (tofu) on-device.

## File toccati
- ruos-desktop/assets/icons/windows-svgrepo-com.svg
- ruos-desktop/assets/icons/windows-svgrepo-com.png (generato)
- ruos-desktop/crates/gui-core/src/images.rs
- ruos-desktop/crates/gui-core/src/desktop/shell.rs
- ruos-desktop/apps/shell/src/lib.rs
