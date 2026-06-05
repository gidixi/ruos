# 287 — Fix: pompa usb::poll() nel loop GUI (input USB in GUI)

**Data:** 2026-06-05

## Cosa
`gfx::fold_mouse` (chiamata dalla GUI ogni frame via `gfx_poll_event`/`pending`)
ora chiama `crate::usb::poll()` prima di drenare la coda mouse.

## Perché
USB è **polled** (no MSI): l'enumerazione e i report HID girano in
`usb_poll_task` dentro l'executor async cooperativo. Una GUI sincrona **possiede
l'executor e non cede** (vedi commento in `wasm/wt/gfx.rs`: la PTY non viene
drenata mentre una sync GUI possiede l'executor), quindi `usb_poll_task` viene
**affamato** mentre la GUI gira → nessun `usb::poll()` → tastiera e mouse USB
morti in GUI. Il mouse PS/2 continua a funzionare perché è **IRQ-driven** (ISR
IRQ12 gira sempre), da cui il sintomo: in GUI PS/2 sì, USB no.

Diagnosi su HW reale (feature `usb-probe`): il mouse USB enumera correttamente
(`slot 2 ... kind=Mouse`), quindi non è un problema di enumerazione; il path
report era solo non servito durante la GUI. Pompare `usb::poll()` dal chokepoint
input della GUI serve tutti i device USB (mouse + tastiera) mentre la GUI è attiva.

## File toccati
- kernel/src/gfx/mod.rs
