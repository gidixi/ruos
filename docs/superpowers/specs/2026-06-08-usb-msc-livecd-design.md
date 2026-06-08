# Design — Live-CD da chiavetta USB: driver USB Mass-Storage (BOT/SCSI) + `/bin` off-boot

**Data:** 2026-06-08
**Stato:** approvato (brainstorming) — pronto per il piano di implementazione

## Problema

Il live-CD (`/bin` letto on-demand da ISO9660 via ATAPI su AHCI, changelog 342–350)
funziona in QEMU q35 e VirtualBox, ma **su hardware reale fallisce con `shell not
found`**.

Causa: su HW reale la ISO viene scritta su **chiavetta USB** (Rufus/dd/Etcher).
La chiavetta è un dispositivo **USB Mass-Storage**, non un CD-ROM ATAPI/AHCI. Il
kernel ha solo driver USB **HID** (tastiera/mouse), nessun driver USB MSC →
`ahci::acquire_atapi_port()` ritorna `None` → l'overlay `/bin` non avviene. Poiché
i `/bin` sono stati tolti da `limine.conf` (off-boot, changelog 348), `/bin` resta
vuoto e `shell.wasm` non si risolve.

In QEMU/VBox funziona perché lì il CD è un ATAPI su AHCI, medium che il kernel sa
leggere.

## Obiettivo

Leggere `/bin` on-demand dalla **chiavetta USB di boot** tramite un driver USB
Mass-Storage nel kernel, mantenendo l'off-boot (RAM bassa). Aggiungere una **rete
di sicurezza**: un set minimo (`shell.cwasm` + init) resta caricato da Limine in
RAM, così il boot raggiunge comunque una shell se il driver USB-MSC fallisce su una
macchina specifica.

### Decisioni di scope

- **Read-only.** `/bin` off-boot si legge soltanto. Niente WRITE SCSI.
- **BOT only.** Bulk-Only Transport (protocol 0x50). UAS (0x62) ignorato: ogni
  flash drive espone anche un'interfaccia BOT.
- **Fallback minimo Limine.** `shell.cwasm` + init ri-aggiunti come moduli Limine;
  gli altri ~80 bin restano off-boot sul filesystem ISO9660.
- **Probe-and-mount per il device di boot.** Dopo l'handoff Limine il kernel
  ri-enumera con il *suo* xHCI: l'handle del boot-disk di Limine non è riusabile.
  Si tenta quindi ISO9660 su ogni LUN MSC e si sceglie quella che parsa e contiene
  `/bin`.

## Architettura

### Cambio del flusso di boot

L'overlay `/bin` esce dalla fase `storage` (che gira **prima** dell'USB) e si
sposta in un nuovo step dopo l'enumerazione USB:

```
boot: ... → storage (/mnt FAT, niente più overlay /bin)
          → usb     (init xHCI; enum async via worklist)
          → media_bin  [NUOVO]  (pompa enum sincrono, monta /bin)
          → userland (shell)
```

- **storage** (`boot/phases/storage.rs`): rimuove la parte overlay `/bin`. Resta
  `modules::mount_all()` (init.sh/init.wasm/root + set minimo /bin) e il mount FAT
  `/mnt`. Il loop di skip della porta CD del boot HBA può restare (non nuoce).
- **usb** (`boot/phases/usb.rs`): invariato — `usb::init()` (bring-up xHCI). L'enum
  vera resta async, drenata da `usb_poll_task`.
- **media_bin** (`boot/phases/media_bin.rs`, NUOVO): vedi sotto.
- **userland**: invariato; lo shell exec ora trova `/bin/shell.cwasm` (overlay USB
  o, in fallback, il modulo Limine in tmpfs).

### Nuovo step `media_bin`

```rust
pub fn init() -> Result<(), BootError> {
    pump_usb_enumeration();                 // while !quiet { usb::poll() }, bounded ~3s
    if overlay_bin_from_atapi() { return Ok(()); }   // path ATAPI esistente, spostato qui
    if overlay_bin_from_usb_msc() { return Ok(()); } // nuovo path USB-MSC
    crate::bwarn!("media_bin", "no removable /bin medium; using Limine fallback set");
    Ok(())
}
```

- `pump_usb_enumeration`: drena la worklist (`usb::poll()`) finché non emerge nulla
  di nuovo o scade un timeout (~3 s). Riusa il pattern già presente in
  `boot/phases/usb.rs::probe` (feature `usb-probe`).
- `overlay_bin_from_atapi`: la closure ATAPI esistente (`acquire_atapi_port` +
  `iso9660::mount_from_blockdev`) **spostata** da `storage.rs`. Non riscritta.
- `overlay_bin_from_usb_msc`: scan `registry` per `SlotKind::Msc`; per ogni LUN
  prova `iso9660::mount_from_blockdev(SectorScale(MscBlock), "/bin", "/bin")`;
  primo successo che contiene `/bin/shell.cwasm` (o struttura `/bin` valida) vince.

L'ordine ATAPI→USB tiene verde il `run-test` esistente (CD su AHCI scelto per primo
in QEMU) e copre la chiavetta su HW reale.

### Infrastruttura bulk transfer xHCI (nuovo `usb/xhci/bulk.rs`)

Oggi xHCI fa control (EP0) + interrupt-IN. MSC richiede bulk-OUT + bulk-IN.

- **Config endpoint bulk**: come `hid::configure_endpoint` ma `EndpointType::BulkOut`
  / `BulkIn`, nessun Interval. Un singolo Configure Endpoint command (type 12) con
  due add-context flag attiva entrambi i DCI bulk.
- **`bulk_xfer(x, slot, dci, ring, buf_phys, len) -> Result<u32, BulkErr>`**:
  sincrono. Enqueue Normal TRB con IOC, suona doorbell(slot, dci), attende
  **Transfer Event** (TRB type 32) via `event::wait_for` con predicato su slot +
  endpoint. Ritorna completion code + residual (word2 bit 0..23). Timeout bounded.
- **Split > 64 KB**: un Normal TRB copre fino a 64 KB (length 17 bit). Transfer più
  grandi = catena di Normal TRB, solo l'ultimo con IOC. I READ(10) per `/bin` sono
  piccoli (≤ 64 KB) → catena minima.
- **STALL recovery**: su CSW phase-error / endpoint halt → Reset Endpoint command +
  Set TR Dequeue Pointer. Minimo (log + un retry). Per read-only su flash è raro.

Logica TRB-build (parole CBW, decode residual) è pura → unit-testabile a parte
dall'hardware.

### Driver USB-MSC (nuovo `usb/msc.rs`)

Protocollo Bulk-Only Transport:

```
CBW (31 B, bulk-OUT) → [data phase bulk-IN/OUT] → CSW (13 B, bulk-IN)
```

- **Rilevamento** in `device::enumerate`/`configure`: oggi si cerca solo HID.
  Aggiungere il match interfaccia **class 0x08 (Mass Storage), subclass 0x06 (SCSI
  transparent), protocol 0x50 (BOT)**, estraendo i due endpoint bulk (IN/OUT) →
  nuovo `registry::SlotKind::Msc(MscState)`.
- **CBW** (LE): signature `USBC`, `dCBWTag`, `dCBWDataTransferLength`,
  `bmCBWFlags` (0x80 = data-IN), `bCBWLUN`, `bCBWCBLength`, `CBWCB[16]` = comando
  SCSI.
- **CSW** (LE): signature `USBS`, `dCSWTag` (match col CBW), `dCSWDataResidue`,
  `bCSWStatus` (0 = pass, 1 = fail, 2 = phase error).
- **SCSI subset (read-only)**:
  - `INQUIRY` (0x12) — sanity/log vendor/product.
  - `TEST UNIT READY` (0x00) — poll ready con retry bounded (flash lente al power-on).
  - `READ CAPACITY(10)` (0x25) — `block_count` + `block_size` (di norma 512).
  - `READ(10)` (0x28) — lettura settori.
  - **No WRITE.**
- **`GET MAX LUN`** (class request, bmRequestType 0xA1, bRequest 0xFE) — di norma 0.
- **`MscState`** (in registry): `slot_id`, `bulk_in_dci`, `bulk_out_dci`, ring(s),
  contatore tag, `block_size`, `block_count`, `max_lun`.

### `MscBlock` → `BlockDevice`

`MscBlock` implementa `blockdev::BlockDevice`:

- `block_size()` / `block_count()` da READ CAPACITY.
- `read_blocks(lba, buf)`: spezza in READ(10) ≤ N settori; per ciascuno costruisce
  CBW, esegue `bulk_xfer` OUT(CBW) → IN(data, in pagina DMA) → IN(CSW); copia i dati
  nel `buf` del chiamante.
- `write_blocks(..)` → `Err(BlockError::Io)` (non supportato).

**Accesso a `Xhci`**: `MscBlock` tiene solo `slot_id` + parametri. Ogni
`read_blocks` prende il lock globale `usb::CTRL` per ottenere `&mut Xhci` (come
`usb::poll`). Così `MscBlock` è `Send`, vive in un `Box<dyn BlockDevice>` montato in
iso9660, indipendente dall'executor.

### Adattatore di scala settore (in `blockdev.rs`)

`iso9660` assume **device LBA == settore ISO da 2048 B** (vale per ATAPI a 2048 B).
USB flash è 512 B → settore ISO N sta al device LBA N × 4. Invece di modificare
`iso9660`:

- **`SectorScale<D>`**: `BlockDevice` con `block_size() = 2048` sopra un device a
  512 B. `read_blocks(lba, buf2048)` → `inner.read_blocks(lba * ratio, buf)` con
  `ratio = 2048 / inner.block_size()`. `block_count = inner_count / ratio`.
- Generico sul ratio; se inner è già 2048 (ATAPI) ratio = 1 → passthrough (uso
  uniforme su entrambi i path).
- `iso9660` resta **invariato**.

Mount USB:
```rust
iso9660::mount_from_blockdev(
    Box::new(SectorScale::new(MscBlock { slot })), "/bin", "/bin")
```

### Set minimo Limine + overlay

- **`limine.conf` + Makefile**: ri-aggiungere coppie `module_path`/`module_cmdline`
  solo per `shell.cwasm` (+ init già presenti), target `/bin/shell.cwasm`. Gli altri
  ~80 bin restano solo in `iso_root/bin/` (filesystem ISO9660), non come moduli.
- `modules::mount_all()` carica il set minimo in tmpfs `/bin`. Poi `media_bin`
  **shadowa** `/bin` con il mount ISO9660 (longest-prefix-match, come oggi). Se
  l'overlay non avviene, resta la shell minima in RAM → boot garantito.

## Componenti (riepilogo) e file toccati

| Componente | File | Tipo |
|---|---|---|
| Bulk transfer xHCI | `kernel/src/usb/xhci/bulk.rs` | NUOVO |
| Driver USB-MSC (BOT/SCSI) | `kernel/src/usb/msc.rs` | NUOVO |
| `SlotKind::Msc` + dispatch | `kernel/src/usb/registry.rs`, `usb/device.rs`, `usb/mod.rs` | mod |
| `MscBlock` BlockDevice | `kernel/src/usb/msc.rs` | NUOVO |
| `SectorScale` adapter | `kernel/src/blockdev.rs` | mod |
| Step `media_bin` | `kernel/src/boot/phases/media_bin.rs` | NUOVO |
| Registra step + ordine | `kernel/src/boot/phases/mod.rs`, `boot/mod.rs` | mod |
| Rimuovi overlay da storage | `kernel/src/boot/phases/storage.rs` | mod |
| Set minimo Limine | `limine.conf`, `Makefile` | mod |
| Target test USB | `Makefile`, `tests/` | mod/NUOVO |

## Testing

**Unit (host `cargo test`, logica pura):**
- `SectorScale`: `Mem` 512 B → LBA tradotti × ratio, `block_count` diviso, ratio = 1
  passthrough.
- MSC: build CBW words (signature/tag/flags/len), decode CSW (status + residual).

**Integrazione QEMU:**
- USB storage emulato: `-device qemu-xhci -device usb-storage,drive=stick
  -drive if=none,id=stick,file=ruos.iso,format=raw`. Esercita bulk + BOT + SCSI +
  iso9660-su-512 B end-to-end.
- **Nuovo target `run-test-usb`**: boot **senza CD ATAPI**, solo USB storage → forza
  il path USB-MSC. Asserzioni seriali:
  - `usb: MSC slot=N bulk_in=.. bulk_out=..`
  - `msc: capacity blocks=.. bsize=512`
  - `media_bin: /bin overlaid from USB-MSC (ISO9660)`
  - stringa di successo esistente (`HELLO`).
- `run-test` attuale (CD ATAPI): resta verde (`media_bin` sceglie ATAPI per primo).
- `boot-checks`: invariati.

**HW reale**: dopo verde QEMU, boot da chiavetta vera. `usb-probe` mostra se la MSC
enumera (`kind=Msc`) e dove diverge (enum vs bulk vs SCSI ready).

## Rischi noti

- Chiavette lente al power-on → retry bounded su `TEST UNIT READY`.
- xHCI reali: timing PED già gestito (`device::reset_root_port`). Bulk su SuperSpeed
  potrebbe richiedere ESIT/max-burst/streams → fuori scope: forziamo no-streams,
  max burst 0 (BOT non usa streams).
- Chiavetta dietro hub su HW reale → enum via route string già supportata
  (`usb/hub.rs`).
- Composite device (MSC + altro): prendere la prima interfaccia BOT, ignorare il
  resto.

## Non-obiettivi

- Niente WRITE/installazione su USB.
- Niente UAS.
- Niente NVMe/altri bus per `/bin` (solo ATAPI + USB-MSC).
- Niente rimozione del fallback Limine (la rete di sicurezza è intenzionale).
