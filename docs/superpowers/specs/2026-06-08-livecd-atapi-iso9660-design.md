# Live-CD: `/bin` overlay da ISO9660 via ATAPI — design

**Data:** 2026-06-08
**Branch:** `feat/livecd-atapi-iso9660`
**Scope:** primo taglio — **solo live-CD completo**. Modalità installata su SSD
rimandata (fallback legacy ai moduli Limine la copre, vedi §7).

## 1. Problema e obiettivo

Oggi **tutti** i bin userspace (~80 `.wasm` CLI + 5 app `.cwasm`, di cui ~45 MB di
app GUI) sono dichiarati come **moduli Limine** in `limine.conf`. Al boot:

1. Limine li carica in RAM (buffer HHDM mappati, mai liberati).
2. `modules::mount_all()` li **ricopia** in tmpfs (heap) ai loro path `/bin/*`.

Risultato: doppia occupazione RAM e boot che pre-carica tutto, usato o no.

**Obiettivo (north-star live-CD):** Limine porta **solo il kernel**. I bin vivono
sul filesystem ISO9660 del CD di boot e il kernel li **monta e legge on-demand**
via un driver ATAPI, senza pre-caricarli in RAM. `/bin` diventa una vista
read-through sul CD.

Questo riprende e completa il lavoro accantonato del branch `livecd`
(commit `a903c01 feat(livecd): dynamic app launcher + apps off boot -> /mnt/apps`),
la cui infra di scan dinamico (`scan_apps`, drop-folder `/mnt/apps`,
`module_by_name` su `/bin` poi `/mnt/apps`) resta valida e viene riusata invariata.

## 2. Vincoli accertati (stato del codice)

- **Storage kernel = solo AHCI/SATA.** `boot/phases/storage.rs` fa
  `pci::find_class(0x01,0x06,0x01)` → AHCI, porta SATA → READ DMA EXT → FAT32 `/mnt`.
- **USB stack = solo HID** (tastiera/mouse). Nessun mass-storage. Dopo il boot il
  kernel non rilegge una chiavetta USB. → il medium live deve essere un **CD ATAPI**
  raggiungibile via AHCI.
- **QEMU `q35 -cdrom $(ISO)`**: la ISO di boot è già un **ATAPI CD-ROM sul controller
  AHCI ICH9 integrato**. Il kernel può rileggere **lo stesso CD da cui ha bootato**.
- **compositor embedded** (`egui_demo.cwasm` via `include_bytes!` in `wm.rs:59`) →
  sempre disponibile. **`shell.cwasm` NON è embedded**: il compositor lo carica da
  `/bin/shell.cwasm` via VFS `module_by_name`, con fallback a egui-demo. → il CD deve
  montare **prima** che parta il compositor.
- **network = kernel-resident** (virtio-net/e1000, smoltcp, DHCP, server SSH). Viene
  su a prescindere dal CD. Solo i *tool CLI* di rete sono bin (lazy dal CD).
- **VFS pluggabile**: `vfs::mount(prefix, FsImpl)` con longest-prefix-match;
  `FsImpl` enum (oggi `Tmpfs`, `Fat32`) in `vfs/fs.rs`. Backend implementano il trait
  `FileSystem` (open/create/unlink/readdir/stat/mkdir/rmdir/rename).
- **`BlockDevice` trait** (`kernel/src/blockdev.rs`) già astrae AHCI port +
  `PartitionDevice`; FAT32 monta da `Box<dyn BlockDevice>`.

## 3. Idea unificante

`/bin` **non viene più da moduli Limine**, ma da un filesystem a blocchi montato.
In modalità live = **ISO9660 sul CD ATAPI**. I consumer (`module_by_name`, shell
PATH, `scan_apps`/launcher) restano **invariati**: continuano a usare `/bin`; la
logica nuova vive tutta nel backend VFS + nella fase di mount (forza dell'approccio
overlay).

## 4. Componenti nuovi (isolati e testabili)

### 4.1 Driver ATAPI — `kernel/src/ahci/atapi.rs`
- **Cosa fa:** legge settori da un dispositivo ATAPI (CD-ROM) collegato a una porta AHCI.
- **Come si usa:** `AtapiDevice::bringup(abar, port_idx) -> Option<AtapiDevice>`;
  implementa `BlockDevice` con settori logici da **2048 B**.
- **Dipende da:** infra AHCI esistente (`hba`, command list/table, FIS).
- **Dettagli:**
  - Riconoscimento porta: signature `PxSIG == 0xEB140101` (ATAPI), vs `0x00000101` (SATA).
    Oggi `port.rs::bringup` assume SATA; va distinto il ramo ATAPI.
  - Lettura: **PACKET** command (`0xA0`) — Command FIS H2D con il bit ATAPI nel
    command header (`CFIS.A`), CDB SCSI nella command table (campo ACMD a 12/16 B):
    `READ(10)` (opcode `0x28`, LBA big-endian, transfer length in blocchi).
  - `read_capacity()`: SCSI `READ CAPACITY(10)` (opcode `0x25`) → ultimo LBA + block size.
  - `BlockDevice::block_size()` ritorna 2048; il VFS/ISO9660 lavora a 2048.
- **Test:** smoke in-boot — leggere settore 16, verificare i 5 byte magic `CD001`
  (analogo all'attuale check `0x55AA` su settore 0 SATA in `storage.rs`).

### 4.2 Filesystem ISO9660 read-only — `kernel/src/vfs/iso9660.rs`
- **Cosa fa:** espone i file del CD come backend VFS read-only.
- **Come si usa:** `Iso9660Fs::mount_from_blockdev(dev, vfs_prefix, iso_base) -> Result<()>`
  → registra `FsImpl::Iso9660` al prefisso VFS dato, mappando i path sotto `iso_base`
  della ISO.
- **Dipende da:** `BlockDevice`, VFS.
- **Dettagli:**
  - Parsa il **Primary Volume Descriptor** (settore logico 16): logical block size,
    location+size del root directory record.
  - Traversata directory via **directory records** (no Joliet/Rock Ridge nel primo
    taglio — nomi 8.3 con `;1` version suffix, sufficienti per `shell.cwasm`,
    `ls.wasm`, ecc.; valutare Joliet se i nomi non bastano → vedi §8).
  - File = **extent contiguo** (location LBA + data length) → lettura semplice.
  - Nuova variante `FsImpl::Iso9660(Iso9660Fs)` in `vfs/fs.rs`: instradare tutti i
    metodi del trait. Scritture (`create/unlink/mkdir/rmdir/rename`) → `EROFS`.
- **Test:** unit su parsing PVD + directory record da buffer fissi (no hardware).

### 4.3 Fase boot CD-mount — in `kernel/src/boot/phases/storage.rs`
- **Cosa fa:** dopo aver portato su l'HBA, per ogni porta distingue SATA vs ATAPI:
  - SATA → `/mnt` FAT32 (come oggi).
  - ATAPI + ISO9660 valido → monta `/bin` dal `/bin` del CD (`iso_base = "/bin"`).
- **Fallback ordinato** (robustezza, niente regressioni):
  1. CD ISO con `/bin` → `/bin` dal CD.
  2. altrimenti `modules::mount_all()` legacy (moduli Limine) — copre boot senza CD
     (es. SSD installato, finché §7 non lo unifica).
- **Ordine fasi:** garantire che il mount `/bin` avvenga **prima** del compositor
  (che gira dopo `fs`/`userland`). Verificare la sequenza in `boot/phases/mod.rs`;
  se necessario montare il CD nella fase `storage` (che già precede `userland`).
- **Due HBA in q35:** la scan deve coprire **tutti** i controller AHCI (ICH9 builtin
  col CD + `-device ahci` con l'hd), non fermarsi al primo `find_class`. → estendere
  `ahci::init` / la scansione PCI a iterare tutti i match classe `0x01/0x06/0x01`.

### 4.4 Build / ISO — `Makefile` + `limine.conf`
- `limine.conf`: rimuovere **tutti** i `module_path: boot():/bin/*` e
  `module_path: boot():/init.wasm`, `/root/*`, `/etc/init.sh`. Restano: il kernel e i
  `/payload/*` (kernel/BOOTX64.EFI/limine.conf/limine-ssd.conf, serviti all'installer).
  → Limine carica solo il kernel.
- `Makefile`: i bin restano staged in `iso_root/bin/` (la ISO è già ISO9660 via
  xorriso) ma **non** più referenziati come moduli. `init.sh`/`init.wasm` su ISO sotto
  i loro path; il kernel li legge dal CD.

## 5. Flusso a regime (live-CD)

```
Limine → carica SOLO kernel → boot fasi:
  arch → mem → interrupts(+SMP) → pci → devices →
  fs (tmpfs / , device files) →
  storage:  AHCI scan tutti i controller →
            SATA?  → /mnt FAT32
            ATAPI ISO? → mount /bin dal CD (ISO9660 RO)
  usb → userland (RNG, net, SSH, executor)
→ compositor parte → module_by_name("/bin/shell.cwasm") → letto dal CD (lazy) → desktop
→ exec CLI (es. `ls`) → open /bin/ls.wasm → read-through dal CD on-demand
```

## 6. Cache (fase 2, opzionale — non bloccante)

Page-cache LRU sui settori 2048 B del CD per evitare riletture ATAPI ripetute
(es. ri-spawn della shell, scan launcher). Fuori dal primo taglio.

## 7. Modalità installata su SSD (fuori scope, ma non rotta)

`install.wasm` copia il payload sull'ESP dell'SSD e si boota con `limine-ssd.conf`,
senza CD. In quel caso il fallback (§4.3) usa `mount_all()` sui moduli Limine. Per
l'unificazione completa ("Limine porta solo il kernel" anche su SSD) servirà:
mettere `/bin/*` sul FAT dell'SSD via installer + montare `/bin` dal FAT quando non
c'è CD. → **spec separata** successiva.

## 8. Rischi e questioni aperte

- **Ordine mount vs compositor** (§4.3): verificare/forzare. Rischio: shell non
  trovata → fallback egui-demo invece del desktop reale.
- **Due HBA AHCI** (§4.3): la scan attuale prende il primo match → potrebbe mancare
  il CD. Da estendere.
- **Nomi file ISO9660**: 8.3 + `;1`. Se i nomi reali (`shell.cwasm`,
  `compositor.cwasm`) eccedono 8.3, serve **Joliet** (SVD a UCS-2) o Rock Ridge.
  Verificare cosa scrive xorriso di default e, se serve, parsare Joliet.
- **UEFI live boot** (OVMF + cdrom): i file su ISO9660 restano leggibili (stessi file
  in `iso_root`). Da confermare in test OVMF.
- **`init.sh`/`init.wasm` off-boot**: oggi moduli; spostati su CD vanno letti dopo il
  mount. Verificare che il primo init non serva prima del mount `/bin`.

## 9. Test (acceptance)

1. **ATAPI smoke (in-boot):** log di settore 16 con magic `CD001`.
2. **ISO9660 unit:** parsing PVD + directory record da buffer fissi.
3. **E2e QEMU live (`make run-test`-style):**
   - asserire mount `/bin` dal CD (nuova riga di log).
   - `/bin/shell.cwasm` caricato dal CD (desktop reale, non fallback egui-demo).
   - eseguire `ls` (bin letto on-demand dal CD).
   - RAM di boot più bassa: nessun modulo `/bin/*` pre-caricato (~45 MB risparmiati).
4. **Non-regressione:** boot senza CD → fallback `mount_all()` → comportamento attuale.

## 10. File toccati (previsti)

- `kernel/src/ahci/atapi.rs` (nuovo)
- `kernel/src/ahci/{mod.rs,port.rs}` (distinzione SATA/ATAPI, scan multi-HBA)
- `kernel/src/vfs/iso9660.rs` (nuovo)
- `kernel/src/vfs/fs.rs` (variante `FsImpl::Iso9660`)
- `kernel/src/boot/phases/storage.rs` (mount `/bin` dal CD + fallback)
- `Makefile`, `limine.conf` (bin off-boot)
- `CHANGELOG/342-26-06-08-livecd-atapi-iso9660.md`
