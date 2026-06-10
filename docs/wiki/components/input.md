# Input (PS/2 + USB HID)

> **Stato:** bozza
> **Aggiornato:** 2026-06-10
> **Fonti:** `kernel/src/keyboard/`, `kernel/src/mouse/`, `kernel/src/usb/`,
> `kernel/src/gfx/mod.rs`

## Cos'è

Il sottosistema di input di ruOS raccoglie **tastiera e mouse** da quattro
sorgenti hardware e li canalizza verso due destinatari: la **shell** (via PTY)
e la **GUI** (via la coda `gfx`). Le sorgenti sono:

- **PS/2 keyboard** (IRQ1) — scancode Set 1
- **PS/2 mouse** (IRQ12) — protocollo IntelliMouse (3 o 4 byte)
- **USB HID boot keyboard** — via xHCI, tradotto in scancode Set 1
- **USB HID boot mouse** — via xHCI, stessa coda `MouseEvent`

Tutte le sorgenti convergono negli stessi path: le keystroke vanno sia al PTY
(per la shell) sia alla coda GUI (`gfx::push_key`), e gli eventi mouse vanno
alla coda condivisa `MouseEvent` (poi foldati in un cursore assoluto dalla GUI).

## Dove vive

| File / cartella | Ruolo |
|-----------------|-------|
| `kernel/src/keyboard/mod.rs` | PS/2 keyboard handler (IRQ1), scancode table |
| `kernel/src/mouse/mod.rs` | PS/2 mouse driver (IRQ12), `MouseEvent` queue |
| `kernel/src/usb/` | xHCI driver + HID boot keyboard/mouse + hub + hot-plug |
| `kernel/src/usb/hid_keyboard.rs` | USB HID → PS/2 Set 1 scancode mapping |
| `kernel/src/usb/hid_mouse.rs` | USB HID boot mouse → `MouseEvent` |
| `kernel/src/gfx/mod.rs` | Coda input GUI, `fold_mouse`, cursore software |

## Modello

```
  PS/2 kbd (IRQ1)    USB HID kbd        PS/2 mouse (IRQ12)   USB HID mouse
       │                  │                     │                   │
       │  scancode Set 1  │ HID usage→Set 1     │  3/4-byte packet  │ boot mouse
       ▼                  ▼                     ▼                   ▼
  ┌──────────────────────────┐          ┌───────────────────────────┐
  │     Keystroke path        │          │     MouseEvent queue      │
  │  gfx::push_key(scancode)  │          │   (delta x/y/buttons/     │
  │  + PTY write (line disc)  │          │    wheel, shared queue)   │
  └──────────┬───────────────┘          └──────────┬────────────────┘
             │                                     │
             ▼                                     ▼
     ┌───────────────┐                    ┌─────────────────┐
     │  Shell / PTY   │                    │ gfx::fold_mouse │
     │ (cooked mode,  │                    │ (delta → abs,    │
     │  echo, ^C)     │                    │  screen clamp)   │
     └───────────────┘                    └────────┬────────┘
                                                   │
                                          ┌────────▼────────┐
                                          │  Compositor /    │
                                          │  GUI service     │
                                          │  (hit-test,      │
                                          │   focus, route)  │
                                          └─────────────────┘
```

## Keyboard

### PS/2 (IRQ1)

L'handler di interrupt legge il byte dal port 0x60 e lo interpreta come
**scancode PS/2 Set 1**: byte singolo per i tasti standard, prefisso `0xE0` per
i tasti estesi (frecce, Home/End, etc.). I modifier (Shift, Ctrl, Alt) sono
tracciati in uno state machine globale.

Il scancode è inviato a:
- **PTY**: tradotto in carattere ASCII (con Shift/Caps), scritto nel master PTY
  attivo (passa per la line discipline: echo, cooked mode, `^C` = VINTR).
- **GUI**: `gfx::push_key(scancode, pressed)` — il compositor/egui lo traduce
  nel proprio input model.

### USB HID

Il driver USB HID boot keyboard (`usb/hid_keyboard.rs`) legge i report a 8 byte
dal device, estrae i keycode HID e li **mappa a scancode PS/2 Set 1** tramite una
tabella di conversione. Da lì seguono lo stesso path del PS/2: PTY + GUI.
La conversione è verificata con boot-check.

## Mouse

### PS/2 (IRQ12)

Il driver PS/2 mouse (`mouse/mod.rs`) probe l'IntelliMouse al boot: se il device
risponde con ID 3, abilita i pacchetti a **4 byte** (con rotellina). Altrimenti
usa pacchetti a 3 byte. Ogni pacchetto contiene:

- **Delta X/Y** (relative, con segno)
- **Pulsanti** (left, right, middle)
- **Wheel** (byte 4, se IntelliMouse: detent con segno)

I delta sono pubblicati nella coda `MouseEvent` condivisa.

### USB HID

Il driver USB HID boot mouse (`usb/hid_mouse.rs`) legge report a 3-4 byte: byte
0 = buttons, byte 1/2 = delta X/Y (con segno), byte 3 = wheel (opzionale).
Pubblica nella stessa coda `MouseEvent`.

### Fold mouse (delta → assoluto)

`gfx::fold_mouse()` somma i delta successivi in un **cursore assoluto**
(`cursor_x`, `cursor_y`), clampato alle dimensioni dello schermo. Il cursore
assoluto è usato dal compositor per hit-test (quale finestra è sotto il
puntatore?) e dalla coda GUI come posizione del mouse.

## Contratti

- **Un'unica coda `MouseEvent`**: PS/2 e USB scrivono nella stessa coda. Il
  compositor è l'unico consumer in modalità GUI; in modalità testo il mouse è
  ignorato.
- **Scancode Set 1** è il formato universale: PS/2 lo genera nativamente, USB HID
  lo produce per conversione. Il codice a valle (PTY, egui) non sa da dove viene.
- **Input layout-agnostic**: il kernel non conosce il layout della tastiera
  (US, IT, DE…). Passa scancode grezzi; la traduzione a carattere è minimale
  (ASCII base, senza dead keys o compose).

## Vincoli e limiti

- **Nessun layout tastiera**: solo ASCII base con Shift per maiuscole/simboli.
  Niente dead keys, compose, AltGr, Unicode input.
- **USB è polled**: il driver xHCI usa polling (niente MSI interrupt). In modalità
  GUI, `usb::poll()` è pompato dal frame loop del compositor — altrimenti la GUI
  (che è sincrona e possiede l'executor) starebbe i dispositivi USB.
- **Boot protocol only**: USB HID usa solo il boot protocol (6KRO keyboard, 3-byte
  mouse). Niente report descriptor parsing, niente NKRO, niente HID complesso.
- **Max 1 tastiera + 1 mouse**: il sistema gestisce più device USB (tramite hub),
  ma tutti scrivono nella stessa coda — non c'è multiplexing per-device.

## Insidie / note

- L'endpoint interval USB è **speed-aware**: low/full-speed device usano un
  encoding diverso da high/super-speed. Senza questo fix, device USB reali dietro
  hub non venivano polled abbastanza spesso.
- Su HW reale, il port-reset USB richiede un **wait for PED** (Port Enable Done):
  il silicon reale setta PED qualche ms dopo il reset-change, a differenza di QEMU
  che lo fa istantaneamente.
- Il BIOS→OS **legacy handoff** (`usb::legacy_handoff`) è necessario su real HW:
  il BIOS possiede il controller xHCI e va esplicitamente rilasciato.

## Vedi anche

- [Boot a fasi](boot-phases.md) — fasi 5 (PS/2) e 8 (USB)
- [Compositor](compositor.md) — input routing e focus
- [Architettura — panoramica](../architecture/overview.md)
- [Indice della wiki](../README.md)
