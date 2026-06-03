# 220 — fix cosmetico: em-dash → `--` nei messaggi installer

**Data:** 2026-06-03

## Cosa
I messaggi stampati di `install`/`mkdisk`/`mkboot` (e i log `binfo`/`bwarn` di
`ruos_install`) usavano l'em-dash `—`, che il font bitmap del framebuffer non ha
→ veniva reso come `???` a schermo (visto su VBox: "to install ??? WIPES that
disk"). Sostituito con `--` nelle stringhe stampate. I commenti `//!` (non
stampati) restano invariati.

## Perché
Leggibilità a schermo durante un'operazione distruttiva.

## File toccati
- user/install/src/main.rs, user/mkdisk/src/main.rs, user/mkboot/src/main.rs
- kernel/src/wasm/host/proc.rs (le 3 stringhe install: refusing / WIPING / ok)
- user-bin/{install,mkdisk,mkboot}.wasm (riccompilati)
