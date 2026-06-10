# Boot a fasi

> **Stato:** bozza
> **Aggiornato:** 2026-06-10
> **Fonti:** `kernel/src/boot/mod.rs`, `kernel/src/boot/phases/`, `kernel/src/main.rs`

## Cos'è

Il boot di ruOS è **sequenziale e deterministico**: dieci fasi eseguite in ordine
fisso, ciascuna con un `init()` che ritorna `Result<(), BootError>`. Se una fase
fallisce, il kernel panic con il messaggio della fase. Non c'è parallelismo nel
boot — ogni fase porta online un sottosistema e le successive lo assumono
disponibile.

L'orchestrazione vive in `boot::run()` (`kernel/src/boot/mod.rs`), chiamato da
`kmain` (`main.rs`) dopo il banner e la calibrazione TSC.

## Dove vive

| File | Ruolo |
|------|-------|
| `kernel/src/main.rs` | Entry point `kmain`, Limine requests, panic handler |
| `kernel/src/boot/mod.rs` | Orchestratore: chiama `init()` in ordine |
| `kernel/src/boot/phases/` | Un modulo per fase: `arch.rs`, `mem.rs`, `interrupts.rs`, `pci.rs`, `devices.rs`, `fs.rs`, `storage.rs`, `usb.rs`, `media_bin.rs`, `userland.rs` |
| `kernel/src/boot/clock.rs` | Boot clock TSC (timestamp pre-LAPIC) |
| `kernel/src/boot/banner.rs` | Banner di avvio |

## Le dieci fasi

```
kmain → boot::run()
  1. arch        → GDT/TSS + IDT
  2. mem         → heap (talc 128 MiB) + frame allocator + paging + ACPI
  3. interrupts  → PIC mask + LAPIC/IOAPIC + timer 100 Hz + SMP bring-up
  4. pci         → PCIe ECAM enumeration
  5. devices     → framebuffer console + GUI geometry + PS/2 kbd+mouse
  6. fs          → VFS + tmpfs + mount moduli Limine in /bin, /etc
  7. storage     → AHCI HBA + SATA + FAT32 /mnt
  8. usb         → xHCI + HID boot kbd+mouse + hub + hot-plug
  9. media_bin   → overlay /bin da ATAPI CD o USB stick (liveCD)
 10. userland    → RNG, networking, service manager, SSH → executor::run() [mai ritorna]
```

### 1. `arch` — CPU foundation

Carica **GDT** (Global Descriptor Table) con un **TSS** (Task State Segment —
l'IST per lo stack delle eccezioni), poi l'**IDT** (Interrupt Descriptor Table)
con handler per eccezioni CPU (divide-by-zero, page fault, double fault, GP
fault, INT3 breakpoint). Dopo questa fase la CPU può prendere eccezioni senza
triple-fault.

### 2. `mem` — memory manager

- **Heap kernel**: `talc` inizializzato su una regione di 128 MiB. Da qui `alloc`
  è disponibile (`Vec`, `Box`, `String`, `BTreeMap`).
- **Frame allocator**: bitmap costruita dalla Limine memory map; API `alloc_frame`
  / `free_frame`.
- **Paging**: `map_page` / `unmap_page`, range MMIO, frame DMA contigue.
- **ACPI**: parse del RSDP → MADT (topologia CPU, APIC ID) e MCFG (base ECAM PCIe).

### 3. `interrupts` — IRQ layer + SMP

- Remappa e maschera il **PIC legacy** (non usato).
- Inizializza **LAPIC** (locale) + **IOAPIC** (da MADT).
- Calibra e avvia il **LAPIC timer** a 100 Hz (su HW reale: calibrazione vs ACPI
  PM timer; su QEMU: PIT-free).
- `STI` — gli interrupt sono abilitati.
- **SMP bring-up**: legge la MP response di Limine, porta ogni AP (Application
  Processor) online con `cpu_id` denso, li parcheggia in `hlt`. L'identità del
  core si legge dal LAPIC ID (workaround VirtualBox: mai `gs:[0]`).

### 4. `pci` — bus enumeration

Enumerazione PCIe via **ECAM** (base da ACPI MCFG). Salva uno snapshot di ogni
dispositivo (BDF, class, subclass, VID:DID, BAR) per i driver successivi (AHCI,
NIC, xHCI).

### 5. `devices` — display + input

- **Framebuffer console**: font bitmap, blend anti-aliased, parser VTE/ANSI,
  scrollback, cursore. Geometry catturata per il servizio GUI.
- **PS/2 keyboard** (IRQ1) + **PS/2 mouse** (IRQ12): probe dell'IntelliMouse per
  la rotellina (pacchetti 4 byte).

### 6. `fs` — filesystem

- **VFS**: mount root tmpfs in-RAM.
- **Moduli Limine**: copiati dal bootloader alla VFS come `/bin/*.wasm`,
  `/etc/init.sh`, ecc. Questi sono i tool userland portati nel ramdisk.
- **Device files**: `/dev/console`, `/dev/null`, `/dev/zero`, `/dev/pts/N`.

### 7. `storage` — disco persistente

- **AHCI HBA**: probe dei controller SATA via PCI, identificazione porte attive
  (IDENTIFY).
- **GPT**: parse della tabella partizioni.
- **FAT32**: mount della partizione dati a `/mnt` (se disponibile). Il driver
  FAT32 supporta lettura/scrittura, `mkfs`, long filenames (LFN).

### 8. `usb` — USB stack

- **xHCI**: init del controller, command ring, event ring, port polling.
- **HID boot**: keyboard + mouse (root port e dietro hub).
- **Hub class driver**: enumerazione ricorsiva.
- **Hot-plug**: attach/detach a runtime.
- Eseguito **dopo** devices (framebuffer) perché i log di bring-up devono essere
  visibili su HW reale (senza seriale).

### 9. `media_bin` — overlay da media rimovibile

- Se il boot avviene da **liveCD** (ATAPI/ISO 9660) o da **USB Mass Storage**,
  copia i `.wasm` dal media in `/bin` (overlay sulla tmpfs).
- Necessario perché il live medium non ha un filesystem leggibile dopo boot (no
  USB MSC driver continuo) — i tool devono stare in RAM.

### 10. `userland` — il sistema a regime

- **RNG**: inizializza il ChaCha20 CSPRNG (seed da `RDRAND`).
- **Networking**: porta online `smoltcp` + il driver NIC (virtio-net / e1000 /
  rtl8169), avvia DHCP.
- **Service manager**: registra i servizi (shell respawn, SSH).
- **SSH server**: avvia `sunset` (ed25519 hostkey, auth password+pubkey).
- **`executor::run()`**: avvia l'executor async **embassy** — **mai ritorna**.
  Da qui in poi il boot è finito e il sistema è a regime: shell su PTY, SSH in
  ascolto, desktop GUI se avviato.

## Contratti

- L'ordine è **vincolante**: `interrupts` ha bisogno di `mem` (per l'IOAPIC MMIO),
  `devices` ha bisogno di `interrupts` (per PS/2 IRQ), `fs` ha bisogno di `mem`
  (per allocare i nodi tmpfs), `storage` di `pci` + `fs`, `usb` di `pci`,
  `media_bin` di `usb` + `fs`.
- Ogni fase che fallisce fa **panic** con `BootError` — non c'è recovery parziale.
- La console framebuffer (fase 5) è il punto da cui i log diventano visibili su
  schermo (prima vanno solo a seriale / boot clock).
- Dopo la fase 10 la console passa a livello WARN+ (INFO solo su seriale + ring
  buffer `dmesg`).

## Vincoli e limiti

- Boot single-core: tutte le fasi girano sul **BSP**. Gli AP sono portati online
  alla fase 3 ma **parcheggiati**; lavorano solo dopo che l'executor li sveglia.
- Nessun timeout di fase: un driver che non risponde blocca il boot (es. xHCI
  reset su HW difettoso).
- I moduli Limine sono copiati interamente in RAM (tmpfs); la dimensione totale
  dei `.wasm` è limitata dalla heap disponibile.

## Insidie / note

- Non rinominare `boot::run()` o le fasi senza aggiornare `main.rs`.
- Il boot clock TSC (`boot/clock.rs`) esiste solo per dare timestamp ai log prima
  che il LAPIC timer sia calibrato (fase 3). Non è usato dopo.
- L'ordine USB (fase 8) prima di `media_bin` (fase 9) è intenzionale: un boot da
  USB stick ha bisogno che xHCI + MSC siano online prima di leggere i file.

## Vedi anche

- [Architettura — panoramica](../architecture/overview.md)
- [Indice della wiki](../README.md)
