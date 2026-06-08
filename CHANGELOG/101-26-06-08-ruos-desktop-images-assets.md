# 101 — Submodule ruos-desktop: pipeline immagini PNG/SVG

**Data:** 2026-06-08

## Cosa
Bump del submodule `ruos-desktop` al commit con la pipeline asset immagini:
decoder PNG (`ruos-assets`, fuori da gui-core), wallpaper da texture con
fallback gradiente, logo/illustrazioni, icone (glifi + power vettoriale IEC 5009),
icona launcher da SVG (xtask SVG→PNG, tinta col tema + hover). gui-core resta puro
(Regola d'oro): il decode sta fuori, gui-core riceve `egui::ColorImage`/texture.

## Perché
Portare nella UI egui di ruos il supporto a immagini (loghi, illustrazioni, icone,
wallpaper), prima assente — nessun path PNG/SVG nel compositor.

## File toccati
- ruos-desktop (submodule pointer → pipeline immagini)
- CHANGELOG/101-26-06-08-ruos-desktop-images-assets.md
