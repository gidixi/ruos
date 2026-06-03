#!/usr/bin/env bash
# Disk-management end-to-end: prove `disks` lists the SATA disks AND that
# `umount /mnt` unblocks `install` onto a disk M1 already auto-mounted at /mnt.
#
# Recipe: build a GPT disk whose data partition (Microsoft-Basic-Data, 0700) is a
# real FAT32 — same as tests/gpt-test.sh — so M1 auto-mounts it at /mnt at boot.
# Then boot ruos with INIT_SCRIPT=user-bin/dm-init.sh, which runs:
#     disks         -> lists the SATA disks as an IDX/MODEL/SIZE table
#     umount /mnt   -> drops the FAT + releases its SATA port
#     install 0     -> PROCEEDS (WIPING + `install: ok`) instead of refusing,
#                      because the /mnt guard now passes (nothing mounted there).
# Boot-marker-only (no mtools at boot). The KEY assertions:
#   - `disks` listed the disk (QEMU HARDDISK / ... MiB),
#   - `umount: /mnt unmounted` appeared,
#   - install did NOT refuse (`install: /mnt is mounted, refusing` MUST be absent),
#   - install proceeded (WIPING / `install: ok`).
# If `umount /mnt` were removed from the init, M1's /mnt would still be mounted
# when `install 0` runs, the guard would fire (`install: /mnt is mounted,
# refusing`), and TEST_FAIL_STILL_REFUSED would trip — that is the negative
# control this test is built around.
set -u
cd "$(dirname "$0")/.."
IMG=build/dm-disk.img
S=build/serial.log
# Stage the FAT image we build with mtools on ext4 /tmp, NOT the build/ dir: the
# repo lives on a 9p /mnt/e mount where mtools' small random writes are slow. We
# dd the finished FAT into the (9p) raw disk image once, sequentially (fast).
FAT=/tmp/dm-data.fat
MARK=/tmp/dm-marker.txt
killq(){ ps -eo pid,comm | awk '/qemu-system/{print $1}' | while read p; do kill -9 "$p" 2>/dev/null||true; done; }
cleanup(){ rm -f "$FAT" "$MARK" 2>/dev/null||true; }
trap cleanup EXIT
killq; sleep 1

# --- build a GPT disk with a FAT32 data partition (like tests/gpt-test.sh) ---
dd if=/dev/zero of="$IMG" bs=1M count=256 status=none
sgdisk -n 1:2048:+1M -t 1:EF00 -c 1:EFI \
       -n 2:0:0      -t 2:0700 -c 2:ruos-data "$IMG" >/dev/null
# sgdisk -i N fields:  "First sector: <lba> (at ..)" / "Partition size: <sectors> sectors (..)"
DLBA=$(sgdisk -i 2 "$IMG" | awk -F': ' '/First sector/{print $2}' | awk '{print $1}')
DSEC=$(sgdisk -i 2 "$IMG" | awk '/Partition size/{print $3}')
[ -n "$DLBA" ] && [ -n "$DSEC" ] || { echo TEST_FAIL_SGDISK; sgdisk -i 2 "$IMG"; exit 1; }
KB=$(( DSEC / 2 ))
rm -f "$FAT"
mkfs.vfat -F 32 -C "$FAT" "$KB" >/dev/null
printf 'hi\n' > "$MARK"
mcopy -o -i "$FAT" "$MARK" ::/HI.TXT
dd if="$FAT" of="$IMG" bs=512 seek="$DLBA" conv=notrunc status=none

# --- boot: M1 mounts /mnt, init runs disks + umount /mnt + install 0 ---
make iso INIT_SCRIPT=user-bin/dm-init.sh > build/dm-iso.log 2>&1 \
  || { echo TEST_FAIL_ISO; tail -20 build/dm-iso.log; exit 1; }
timeout 180 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom build/os.iso \
  -serial stdio -display none -no-reboot -m 1024 -device qemu-xhci \
  -drive file="$IMG",format=raw,if=none,id=d0 -device ahci,id=ahci \
  -device ide-hd,drive=d0,bus=ahci.0 > "$S" 2>&1 & QP=$!
for _ in $(seq 1 80); do
  grep -qF "dm-done" "$S" 2>/dev/null && break
  kill -0 "$QP" 2>/dev/null || break
  sleep 2
done
killq
cp "$S" build/serial.dm.log

# --- asserts ---
# M1 auto-mounted the FAT data partition at /mnt (boot-time precondition).
grep -qF "mnt mounted FAT" build/serial.dm.log \
  || { echo TEST_FAIL_NO_MNT; tail -30 build/serial.dm.log; exit 1; }
# `disks` rendered its table. Assert BOTH the tool's header line `IDX  MODEL ...
# SIZE` (only the `disks` tool emits it — the kernel ahci logs never do) AND a
# disk data row (QEMU's IDENTIFY model "QEMU HARDDISK" + a "<N> MiB" size). The
# header proves the tool ran; the row proves it listed the SATA disk.
grep -qE "IDX +MODEL +SIZE" build/serial.dm.log \
  || { echo TEST_FAIL_DISKS_HDR; tail -30 build/serial.dm.log; exit 1; }
grep -qiE "QEMU HARDDISK +[0-9]+ MiB" build/serial.dm.log \
  || { echo TEST_FAIL_DISKS; tail -30 build/serial.dm.log; exit 1; }
# NO-CORRUPTION proof: dm-init runs `cat /mnt/HI.TXT` AFTER `disks` (while /mnt is
# still mounted). If `disks` had re-brought-up the mounted port (the bug), it would
# have reprogrammed that port's PxCLB/PxFB and the subsequent /mnt read would read
# garbage / fail. So assert the file's content `hi` appears in the serial AND that
# it comes AFTER the `disks` table header — i.e. reading /mnt still works once
# `disks` has run against the mounted disk. (HI.TXT == "hi"; written by mcopy above.)
# Strip the PTY's trailing CR (serial goes through a PTY → lines end "\r\n") so
# the FAT content "hi" matches as a whole line regardless of the carriage return.
awk '
  { sub(/\r$/, "") }
  /IDX +MODEL +SIZE/ { seen_disks=1 }
  seen_disks && $0 == "hi" { found=1 }
  END { exit(found ? 0 : 1) }
' build/serial.dm.log \
  || { echo TEST_FAIL_MNT_CORRUPTED_AFTER_DISKS; tail -30 build/serial.dm.log; exit 1; }
# `umount /mnt` succeeded and released the port.
grep -qF "umount: /mnt unmounted" build/serial.dm.log \
  || { echo TEST_FAIL_UMOUNT; tail -30 build/serial.dm.log; exit 1; }
# The install /mnt guard MUST NOT have fired (this is the whole point).
grep -qF "install: /mnt is mounted, refusing" build/serial.dm.log \
  && { echo TEST_FAIL_STILL_REFUSED; tail -30 build/serial.dm.log; exit 1; }
# install actually proceeded onto the disk (WIPING is the kernel install log;
# `install: ok` is the tool's success line).
grep -qiE "WIPING|install: ok" build/serial.dm.log \
  || { echo TEST_FAIL_NO_INSTALL; tail -30 build/serial.dm.log; exit 1; }

echo TEST_PASS_DM
