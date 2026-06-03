# ruos boot script — disk-management test (tests/disk-mgmt-test.sh).
# Booted with a GPT disk whose data partition is a real FAT32, so M1 auto-mounts
# it at /mnt at boot. Then:
#   disks       lists the SATA disks as a clean IDX/MODEL/SIZE table (proves the
#               disk enumeration tool, replacing the kernel log `install` spat).
#   umount /mnt unmounts the FAT and releases its backing SATA port, so the
#               `install` /mnt guard (which refuses while /mnt is mounted) passes.
#   install 0   now PROCEEDS onto SATA disk 0 (WIPING + `install: ok`) instead of
#               refusing — proving `umount` unblocks a (re)install onto a ruos disk.
# The host-side test greps serial for the disks line, `umount: /mnt unmounted`,
# and that install proceeded (NOT `install: /mnt is mounted, refusing`).
echo ruos boot OK
disks
umount /mnt
install 0
echo dm-done
