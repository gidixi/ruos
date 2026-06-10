# VFS / Storage

> **Stato:** bozza
> **Aggiornato:** 2026-06-10
> **Fonti:** `kernel/src/vfs/`, `kernel/src/ahci/`, `kernel/src/gpt.rs`,
> `kernel/src/disk.rs`, `kernel/src/blockdev.rs`, `kernel/src/crc32.rs`

## Cos'è

Il sottosistema VFS/storage di ruOS è la **gerarchia dei file** vista da tutti i
task WASM. Ha due facce:

- **VFS** — l'albero virtuale: trait `FileSystem`/`Inode`/`File`, mount point,
  risoluzione path. La root `/` è un **tmpfs** (in-RAM, volatile).
- **Storage** — il driver fisico: AHCI/SATA, GPT, un driver **FAT32 nativo**
  montato a `/mnt` per la persistenza.

Ogni tool WASM vede un unico namespace unificato: `/bin` (tmpfs), `/mnt` (FAT32),
`/dev/*` (device file), `/etc` (tmpfs). La distinzione tra i backend è trasparente.

## Dove vive

| File / cartella | Ruolo |
|-----------------|-------|
| `kernel/src/vfs/mod.rs` | Trait `FileSystem`, `Inode`, `File`; mount table; risoluzione path |
| `kernel/src/vfs/tmpfs.rs` | Tmpfs: in-RAM, root `/`, nodi dinamici |
| `kernel/src/vfs/fat32.rs` | Driver FAT32 nativo: R/W, `mkfs`, long filenames (LFN) |
| `kernel/src/vfs/devfs.rs` | Device files: `/dev/console`, `/dev/null`, `/dev/zero`, `/dev/pts/N` |
| `kernel/src/ahci/` | Driver SATA: HBA, command list, FIS, porta SATA |
| `kernel/src/blockdev.rs` | Trait `BlockDevice` (read/write sector) |
| `kernel/src/gpt.rs` | Parse/author GPT (protective MBR + primary/backup header + entries) |
| `kernel/src/disk.rs` | Disk authoring (`mkdisk`), boot tree copy, installer |
| `kernel/src/crc32.rs` | CRC32 per GPT e gzip |

## Modello

```
                       VFS
         ┌──────────────┼───────────────┐
         │              │               │
      tmpfs (/)     FAT32 (/mnt)    devfs (/dev)
         │              │               │
   /bin/*.wasm     /mnt/bin/*      /dev/console
   /etc/init.sh    /mnt/passwd     /dev/null
                   /mnt/auth.key   /dev/zero
                                   /dev/pts/0..7
```

### Trait principali

- **`FileSystem`** — un backend mountabile: `open`, `create`, `mkdir`, `remove`,
  `rename`, `stat`, `readdir`.
- **`Inode`** — un nodo nell'albero: tipo (file/dir/device), dimensione, figli.
- **`File`** — un handle aperto: `read`, `write`, `seek`, `close`.

### Mount table

Un `BTreeMap<String, Arc<dyn FileSystem>>` mappa mount point → backend. La
risoluzione path scandisce i componenti e seleziona il backend più specifico.

### Tmpfs

In-RAM, `Vec<u8>` per il contenuto dei file, `BTreeMap<String, Inode>` per le
directory. Ogni scrittura alloca/rialoca. Non c'è persistenza — tutto scompare
al reboot. I moduli Limine (`.wasm`, `init.sh`) vengono copiati qui alla fase 6
del boot.

### FAT32

Driver nativo (non libreria esterna): parse del BPB, FAT table (read/write),
directory entries (8.3 + LFN), cluster chain follow, allocazione/deallocazione
cluster. Supporta `mkfs` (formattazione di una partizione in FAT32 da zero).
Il driver LFN (long filename) è essenziale per i tool con nomi > 8 char.

### Device files

- `/dev/console` — lettura: drain stdin del PTY; scrittura: `kprintln!`.
- `/dev/null` — sink; lettura: EOF.
- `/dev/zero` — lettura: byte zero infiniti.
- `/dev/pts/N` — device PTY (vedi [SMP/executor](smp-executor.md) per i dettagli).

## Storage AHCI/SATA

Il **driver AHCI** (`ahci/`) enumera i controller SATA dalla lista PCI, probe
le porte attive, e per ogni porta esegue un `IDENTIFY DEVICE` (o `IDENTIFY PACKET
DEVICE` per ATAPI/CD-ROM). La porta SATA è il `BlockDevice` che il driver FAT32
usa per leggere/scrivere settori.

### GPT

`gpt.rs` parse e **autori** tabelle GPT complete: protective MBR, header primario
e di backup, entries con tipo GUID (EFI System Partition, Microsoft Basic Data).
CRC32 verificati in lettura, calcolati in scrittura.

### Disk authoring e install

`disk.rs` orchestra la creazione di un disco bootabile:

1. **`mkdisk`**: autori GPT + formatta ESP (FAT32) + data partition (FAT32).
2. **`mkboot`**: mkdisk + copia dell'albero di boot (kernel, BOOTX64.EFI,
   limine.conf, moduli `.wasm`).
3. **`install`**: mkboot mirato a un disco scelto, con guard che rifiuta se `/mnt`
   è montato (evita di sovrascrivere il disco da cui si è avviati).

## Contratti

- Il VFS è l'**unica interfaccia file** per il WASM: le host fn `path_open`,
  `fd_read`, `fd_write`, `readdir` passano tutte per la VFS.
- I path sono **capability-scoped**: un task non può uscire dalla sua root `/`
  con `../`.
- Il driver FAT32 ha bisogno che l'AHCI sia online (fase 7 del boot) e che il GPT
  sia parseato.

## Vincoli e limiti

- **Tmpfs volatile**: tutto ciò che è in `/` (incluso `/bin`) scompare al reboot.
  La persistenza è solo su `/mnt` (FAT32 su SATA).
- **FAT32 solo**: nessun ext4, btrfs, o altro filesystem. LFN supportato ma
  niente case-sensitivity nativa (FAT è case-insensitive).
- **No symlink**: il VFS non ha un tipo inode symlink.
- **No permessi**: nessun modello uid/gid/mode; `chmod`/`chown` non esistono.
- **No timestamp**: `stat` ritorna dimensione e tipo, ma non mtime/ctime/atime.

## Insidie / note

- `readdir` del VFS ritorna le entries in ordine di `BTreeMap` (tmpfs) o ordine
  FAT (FAT32) — non è garantito ordinamento lessicografico.
- Il mount a `/mnt` avviene alla fase 7: i tool eseguiti prima (es. `init.sh`
  fase 10) non vedono `/mnt` se il disco non è pronto.
- `umount /mnt` è necessario prima di `install` — il guard dell'installer rifiuta
  se il mount è attivo.

## Vedi anche

- [Boot a fasi](boot-phases.md) — fasi 6 (fs) e 7 (storage)
- [Architettura — panoramica](../architecture/overview.md)
- [Indice della wiki](../README.md)
