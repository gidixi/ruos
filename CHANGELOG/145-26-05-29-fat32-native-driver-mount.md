# 145 — FAT32 native driver + /mnt mount (Step 15 closeout)

**Data:** 2026-05-29

## Cosa

Driver FAT32 native in-tree (`kernel/src/vfs/fat32.rs` ~500 LoC). Zero
dipendenze esterne. Sostituisce il tentativo `fatfs` crate (0.3 needs
`core_io` broken; git HEAD log version conflict embassy-executor).

### Read path
- `Bpb::parse` da sector 0: bytes/sec, sec/cluster, rsvd_sec, num_fats,
  fat_sz32, root_clus, tot_sec32
- `Inner::chain(start)` segue FAT entries finché EOC (0x0FFF_FFF8+),
  caching FAT sector cross consecutive lookups
- `read_dir_entries` scansiona cluster directory: skip LFN (attr=0x0F),
  volume label, deleted (0xE5), end-of-dir (0x00). 8.3 normalizzato
  lowercase.
- `Fat32File::read` itera cluster chain, RMW per cluster

### Write path
- `alloc_cluster` linear FAT scan da cluster 2, mark EOC, zero data
- `write_fat_entry` mirror su ogni FAT copy (num_fats=2 standard)
- `Fat32File::write` estende chain on demand, RMW per cluster, update
  dir entry size + first_cluster
- `create_file` alloc first cluster, encode 8.3 short name, free slot
  parent dir

### VFS integration
- `FsImpl::Fat32(Fat32Fs)` variant in `vfs/fs.rs`
- `FileImpl::Fat32(Fat32File)` in `vfs/file.rs`
- `VfsError::{IoError, Unsupported}` aggiunti
- `vfs::resolve` ora longest-prefix mount lookup (era hardcoded mount 0)
- `boot/phases/storage.rs` chiama `fat32::mount_from_ahci_port`

### Limitazioni (deferred)
- Solo nomi 8.3 corti (LFN deferred)
- `unlink`/`mkdir`/`rmdir`/`rename` → `Unsupported`
- Truncate semantica minima (size=0, chain non liberata)
- Directory expansion single-cluster

## Test

`make run-test` → TEST_PASS. Serial:
```
fat32 BPB ok rsvd=32 fats=2 fatsz=1009 clus_sec=1 root_cluster=2 tot_sec=131072
fat32 mnt mounted FAT
hello from disk                <- cat /mnt/hello.txt da init.sh
# ruos boot script             <- cat /mnt/init.bak (cp'd da /etc/init.sh)
echo ruos boot OK
...
```

mtools post-test verifica:
```
INIT     BAK       624 bytes  ← file persistito su disk.img
```

End-to-end pipeline:
```
shell.wasm path_open(/mnt/init.bak, CREATE|WRITE)
  → VFS resolve longest-prefix → FsImpl::Fat32
  → Fat32Fs::open → create_file (alloc cluster + dir entry)
  → Fat32File::write (cluster RMW + update_entry_on_disk)
  → AHCI WRITE DMA EXT → SATA disk
```

Step 15 MVP completo: AHCI + FAT32 + /mnt R/W.

## File toccati

- kernel/src/vfs/fat32.rs (nuovo)
- kernel/src/vfs/mod.rs (`pub mod fat32` + longest-prefix resolve)
- kernel/src/vfs/fs.rs (FsImpl::Fat32 variant + forwarders)
- kernel/src/vfs/file.rs (FileImpl::Fat32 variant + dispatch)
- kernel/src/vfs/error.rs (IoError + Unsupported)
- kernel/src/boot/phases/storage.rs (mount /mnt dopo bring-up)
- user-bin/init.sh (cat /mnt/hello.txt + cp/cat write smoke)
- Makefile (run-test gate `mnt mounted FAT`, `hello from disk`)
- CHANGELOG/145-26-05-29-fat32-native-driver-mount.md (questo)
