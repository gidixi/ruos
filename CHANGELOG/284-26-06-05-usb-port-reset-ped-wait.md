# 284 — Fix: USB port reset attende PED su hardware reale

**Data:** 2026-06-05

## Cosa
In `reset_root_port` (kernel/src/usb/device.rs), dopo che il reset completa (PRC
settato), si attende ora **PED (Port Enabled/Disabled) = 1** con un poll bounded
(100 ms) prima di dichiarare la porta enabled, invece di leggere PED una sola
volta subito dopo aver pulito PRC.

## Perché
Su xHCI reale il controller alza PED un breve ritardo (controller-specific) DOPO
aver alzato PRC — non simultaneamente come fanno QEMU e VirtualBox. La lettura
immediata di PED restituiva `enabled=false` anche se la stessa porta leggeva
`ped=1` pochi ms più tardi. Risultato: `reset_root_port` ritornava `None`,
`enumerate` non veniva mai chiamato, nessun device USB enumerava, e il ciclo
connect→reset andava in retry infinito.

Diagnosi via feature `usb-probe` (changelog 283) su hardware reale: lo scrollback
mostrava `enabled=false reset_done=true` durante il reset, ma il summary post-reset
(3 s dopo) mostrava le stesse porte connesse con `ped=true pls=0` ed `enumerated
slots: (none)`. La contraddizione (PED falso al reset, vero dopo) ha individuato
il timing PRC→PED come causa.

## File toccati
- kernel/src/usb/device.rs
