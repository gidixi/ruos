# USB HID boot mouse — design

**Data:** 2026-06-05
**Stato:** implementato (verificato in QEMU; da confermare su HW reale)

## Obiettivo

Far funzionare un mouse USB esterno: enumerarlo via xHCI, riceverne i report HID
boot-protocol, e muovere il cursore della GUI — accanto (non al posto di) al mouse
PS/2 e alla tastiera USB già esistenti.

## Contesto

Lo stack USB (`kernel/src/usb/`) enumera i device e dispatcha la sola tastiera HID
boot (interface class 3, subclass 1, protocol 1). Il mouse PS/2
(`kernel/src/mouse/`) decodifica pacchetti 3-byte in `MouseEvent` e li spinge in
una coda drenata da `gfx::fold_mouse`, che piega i delta in una posizione assoluta
e ridisegna il cursore. Mancava il mouse USB.

## Decisioni

- **Boot protocol, non report-descriptor parsing.** Report fisso
  `[buttons][dx i8][dy i8](wheel)`. Specchia il path tastiera. YAGNI: niente
  parser HID generico, niente scroll/multitouch ora.
- **Routing nella stessa coda del PS/2.** Il report mouse USB viene decodificato
  in `crate::mouse::MouseEvent` e iniettato con `crate::mouse::inject` nella coda
  che la GUI già drena. Zero modifiche alla GUI; USB e PS/2 coesistono.
- **Riuso del setup endpoint.** `hid::configure_endpoint` (interrupt-IN +
  SET_PROTOCOL boot) è identico per tastiera e mouse; differiscono solo la
  lunghezza del report e il parsing. Aggiunto `report_len` a `HidState`.
- **Convenzione Y.** USB HID riporta +Y = giù, che è già la convenzione di
  `MouseEvent` → nessuna negazione (a differenza del PS/2, che nega).
- **Path tastiera invariato.** Vincolo: la tastiera (funzionante su HW reale)
  resta `report_len = 8` e `on_report` intatto.

## Componenti

| Unità | Responsabilità |
|---|---|
| `usb/mouse.rs::decode_boot_mouse(&[u8]) -> MouseEvent` | Decode puro (unit-test) |
| `mouse::inject(MouseEvent)` | Wrapper pub sulla coda condivisa |
| `hid::HidBootEndpoint { …, proto }` | Descrittore endpoint HID boot (1=kbd, 2=mouse) |
| `hid::configure_endpoint` | Setup interrupt-IN + boot protocol (condiviso) |
| `hid::on_report_mouse` | Decode + inject + re-queue TRB |
| `registry::SlotKind::Mouse(HidState)` | Tipo slot mouse; dispatch + teardown |
| `device::configure` / `enumerate` | Rileva proto 1/2, dispatcha kbd vs mouse |

## Flusso dati

```
mouse USB → endpoint interrupt-IN → xHCI transfer event → usb::poll()
  → registry::dispatch_transfer → hid::on_report_mouse
  → decode_boot_mouse → mouse::inject → coda mouse
  → gfx::fold_mouse → posizione assoluta + GfxEvt → cursore
```

## Test

- **Unit:** `usb::mouse::self_test()` (boot-checks) — bottoni, sign-extension i8
  su entrambi gli assi, no-negazione Y, report corto.
- **Integrazione (QEMU):** `-device usb-mouse` enumera `kind=Mouse`; QMP
  `input-send-event` rel → `mouse events injected > 0` (path report completo).
- **Accettazione:** mouse USB su HW reale muove il cursore GUI.

## Fuori scope (per ora)

Scroll wheel, tasti extra (>3), mouse ad alta risoluzione, touchpad I2C-HID,
report-descriptor parsing.
