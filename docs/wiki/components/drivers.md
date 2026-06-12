# Hardware Abstraction & Driver

> **Stato:** bozza
> **Aggiornato:** 2026-06-11
> **Fonti:** `kernel/src/pci/`, `kernel/src/usb/`, `kernel/src/ahci/`, `kernel/src/net/`

## Cos'è

Essendo un sistema operativo interamente asincrono e cooperativo, ruOS gestisce l'hardware in modo diverso dai classici OS basati su interrupt preemptivi.

L'astrazione hardware (driver) in ruOS evita il più possibile la proliferazione di handler IRQ asincroni che interrompono il kernel, preferendo invece architetture di "polling controllato" tramite l'executor `embassy` o meccanismi ibridi, con un grande accento sul bus **PCIe**.

## Dove vive

| File / cartella | Ruolo |
|-----------------|-------|
| `kernel/src/pci/` | Scansione e configurazione del bus PCI Express |
| `kernel/src/ahci/` | Driver per dischi SATA (storage) |
| `kernel/src/usb/` | Driver per controller xHCI e stack USB (in sviluppo) |
| `kernel/src/net/` | Driver di rete (es. Realtek `rtl8139`, Intel `e1000`) |
| `kernel/src/keyboard/`, `mouse/` | Input PS/2 legacy |

## Modello Async Polling vs Interrupt

In un sistema operativo monolitico tradizionale, quando un pacchetto di rete arriva o un disco finisce di leggere, la scheda madre invia un IRQ. Il processore ferma immediatamente il programma corrente, salta nell'Interrupt Service Routine (ISR) del driver, processa i dati e ritorna.

In **ruOS**, l'uso degli ISR è ridotto al minimo indispensabile (principalmente il timer a 100 Hz e la tastiera PS/2). I driver PCIe come quello di Rete o l'USB xHCI sono spesso incapsulati in **Task Async**:
1. Il task esegue un loop `.await`.
2. Quando il device ha bisogno di attenzione, il task viene svegliato (spesso tramite timer periodico o poll).
3. Il task legge i ring buffer del device (es. descrittori DMA).
4. Cede volontariamente la CPU all'executor.

Questo previene l'effetto "interrupt storm" e rende il kernel completamente deterministico e sicuro rispetto allo stato mutabile.

## Il Bus PCIe

Alla fase 4 del boot, il kernel scansiona il bus **PCI Express**. Non c'è un file speciale nel VFS per l'accesso crudo al bus: tutto avviene a livello kernel.

I dispositivi supportati (come l'AHCI per i dischi e le NIC supportate) vengono individuati tramite Vendor ID e Device ID. Il kernel mappa in memoria i loro registri MMIO (Memory-Mapped I/O) (spesso con `page_flags::NOCACHE` per disabilitare la cache della CPU su quelle aree).

## DMA (Direct Memory Access)

I driver ruOS allocano pesantemente frame fisici contigui (`dma::alloc()`) per permettere ai dispositivi hardware di scambiare dati direttamente con la RAM, bypassando la CPU.

Questa operazione in Rust `no_std` richiede attenzione estrema:
- I buffer DMA non devono mai essere spostati, perciò vengono "pinnati".
- Il driver traduce gli indirizzi virtuali (HHDM) forniti da Rust negli indirizzi fisici grezzi richiesti dall'hardware.

## Storage: AHCI e GPT

Il driver **AHCI** mappa le porte SATA attive. Interagisce con i dischi asincronamente inviando i Command Header in memoria DMA. Non essendoci polling bloccante, una lettura su disco sospende il Fiber del task chiamante finché il DMA non conclude l'operazione (e il task di polling AHCI non notifica l'evento). Il filesystem FAT32 opera su questa astrazione a blocchi.
