//! Phase — Storage: discover the AHCI HBA + bring up SATA ports.
//!
//! Runs after PCI and devices, before userland. Non-fatal: machines without
//! SATA log a warning and continue (no `/mnt`).

use crate::boot::BootError;
use crate::blockdev::BlockDevice;

pub fn init() -> Result<(), BootError> {
    // Popola lo userspace e poi sovrappone `/bin` dal CD live, se presente.
    // Eseguito in OGNI caso (anche senza HBA AHCI), come closure così da poterlo
    // chiamare sia nel path "HBA assente" sia DOPO `ahci::init()`.
    let mount_userspace = || {
        // Carica i moduli Limine in tmpfs: init.sh, init.wasm, /root e il set
        // minimo /bin (shell.wasm + bootstrap) come rete di sicurezza. Il resto
        // di `/bin` è off-boot: lo sovrappone la fase `media_bin` (dopo l'USB)
        // leggendo l'ISO9660 da CD ATAPI o chiavetta USB. Spostato lì perché
        // l'USB-MSC si enumera DOPO questa fase.
        let n = crate::modules::mount_all();
        crate::binfo!("storage", "mounted {} boot modules into tmpfs", n);
    };

    let hba = match crate::ahci::init() {
        Some(h) => h,
        // Nessun HBA AHCI: niente SATA e niente CD. Popola comunque lo userspace
        // (moduli Limine) prima di uscire.
        None    => {
            mount_userspace();
            return Ok(());
        }
    };

    // HBA presente e `BOOT_HBA` popolato da `ahci::init()`: ora `acquire_atapi_port`
    // (che legge `BOOT_HBA` + scan multi-HBA) può trovare il CD.
    mount_userspace();

    // Walk Ports-Implemented; bring up every populated SATA port.
    for idx in 0..32 {
        if (hba.pi & (1 << idx)) == 0 { continue; }
        // Skip the port already owned by the live-CD `/bin` ISO9660 mount: a
        // second bringup here would reprogram that live port's command list base
        // and corrupt the in-flight CD reads (VirtualBox: CD on boot-HBA port 0).
        if crate::ahci::boot_cd_port() == Some(idx as usize) { continue; }
        if let Some(mut port) = crate::ahci::AhciPort::bringup(hba.abar, idx as usize) {
            // An ATAPI device (CD-ROM) is not a FAT disk — never try to /mnt it
            // (its 2048 B blocks also reject the 512 B sector-0 read below).
            if port.is_atapi { continue; }
            // Smoke: read sector 0 (FAT BPB) + confirm 0x55AA boot signature
            // at bytes 510..512. End-to-end proof that READ DMA EXT works
            // against the QEMU disk we formatted with mkfs.vfat.
            let mut buf = alloc::vec![0u8; 512];
            match port.read_blocks(0, &mut buf) {
                Ok(()) => {
                    let sig = u16::from_le_bytes([buf[510], buf[511]]);
                    if sig == 0xAA55 {
                        crate::binfo!(
                            "ahci", "disk read OK sector 0 boot_sig=0x{:04x} oem={:?}",
                            sig,
                            core::str::from_utf8(&buf[3..11]).unwrap_or("?"),
                        );
                    } else {
                        crate::bwarn!("ahci", "sector 0 read but no FAT sig (got 0x{:04x})", sig);
                    }
                }
                Err(e) => crate::bwarn!("ahci", "sector 0 read failed: {}", e),
            }
            // Mount the FAT32 volume at /mnt. Failures log and continue —
            // boot still completes with tmpfs at /.
            //
            // Parse the GPT first: if present, mount the data partition; else
            // fall back to a raw FAT at LBA 0. We copy out (base,count) from the
            // owned GptPartition before moving `port`, so the mutable borrow
            // taken by `parse` has ended by the time we box the port.
            let data_part: Option<(u64, u64)> = crate::gpt::parse(&mut port)
                .and_then(|parts| crate::gpt::find_data(&parts).map(|d| (d.first_lba, d.sectors())));
            let mounted = match data_part {
                Some((base, count)) => {
                    crate::binfo!("storage", "gpt: data part lba={} sectors={} -> /mnt", base, count);
                    let pd = crate::blockdev::PartitionDevice::new(
                        alloc::boxed::Box::new(port), base, count);
                    crate::vfs::fat32::mount_from_blockdev(alloc::boxed::Box::new(pd))
                }
                None => crate::vfs::fat32::mount_from_blockdev(alloc::boxed::Box::new(port)),
            };
            match mounted {
                Ok(())  => {
                    // Record the live /mnt port so the later `media_bin` phase's
                    // ATAPI scan does NOT re-bringup it — a second bringup would
                    // reprogram this port's PxCLB/PxFB and corrupt the mounted
                    // FAT's in-flight DMA (the reorder moved acquire_atapi_port
                    // to AFTER this mount).
                    crate::ahci::set_mounted_sata_port(idx as usize);
                    crate::binfo!("fat32", "mnt mounted FAT");
                }
                Err(e)  => crate::bwarn!("fat32", "mount /mnt failed: {}", e),
            }
            break;
        }
    }
    Ok(())
}
