# 204 — USB: BIOS→OS handoff + fase dopo il framebuffer (boot HW reale)

**Data:** 2026-06-02

## Cosa
Fix del boot su **hardware reale**: con la fase USB attiva, la macchina reale si
bloccava (Limine carica i moduli → schermo nero, nessun log). Due cause + fix:

1. **Mancava il BIOS→OS handoff dell'xHCI.** Su HW reale il firmware possiede il
   controller (USB legacy keyboard/boot). Resettarlo/avviarlo mentre il BIOS lo
   possiede ancora — col suo handler SMI attivo — fa combattere l'**SMM** per il
   controller e la macchina si **congela** (l'SMM preempta l'OS, i timeout bounded
   non bastano). QEMU non ha extended capabilities né SMM → non si vedeva.
   Fix: `bios_handoff()` percorre la Extended Capability list (raw MMIO), trova
   USB Legacy Support (id 1), setta l'OS-owned semaphore, aspetta (bounded 100ms)
   che il BIOS rilasci, poi scrive USBLEGCTLSTS=0xE000_0000 (tutti gli SMI enable
   a 0, i 3 bit di stato RW1C azzerati) così il firmware non può più alzare SMI.

2. **La fase USB girava PRIMA della console framebuffer.** Su HW reale (senza
   seriale) non si vedeva nessun log durante il bring-up USB → schermo nero.
   Fix: spostata `phases::usb::init()` dopo `devices` (framebuffer) / `storage`,
   prima di `userland`. USB dipende solo da PCI. Ora i log del bring-up USB sono
   VISIBILI su HW reale (diagnosi futura).

## Perché
La macchina reale non bootava più dopo l'aggiunta della fase USB. QEMU mascherava
entrambi i problemi (no SMM, e si vedono i log via seriale stdio).

## File toccati
- kernel/src/usb/xhci/mod.rs (bios_handoff + chiamata; usa size del BAR)
- kernel/src/boot/mod.rs (ordine fasi: usb dopo storage)

## Note
- Verificato su QEMU: TEST_PASS, USB enumera ancora (handoff no-op senza ext cap).
- Da testare su HW reale: ora i log USB compaiono sul framebuffer; l'handoff
  dovrebbe togliere il freeze SMM. Se ancora si blocca, il log mostra dove.
- **Stesso bug è su main** (USB MVP gira USB prima del framebuffer, senza
  handoff) — questo fix va portato anche lì al merge del branch.
