#!/usr/bin/env bash
# M2a end-to-end: prove ruos can AUTHOR a real disk (GPT + FAT32 + /EFI/BOOT)
# and that M1 can BOOT + auto-mount it (round-trip persistence). Two phases on
# the SAME disk image, two `make iso INIT_SCRIPT=...` builds (the ruos shell has
# no conditionals, so a single init can't branch; re-running mkdisk on phase 2
# would re-wipe). No host loop devices — QEMU AHCI is the only disk path.
set -u
cd "$(dirname "$0")/.."
IMG=build/m2a-disk.img; S=build/serial.log
killq(){ ps -eo pid,comm | awk '/qemu-system/{print $1}' | while read p; do kill -9 "$p" 2>/dev/null||true; done; }
boot(){ # $1=timeout secs ; boots build/os.iso with $IMG as the only AHCI disk
  timeout "$1" qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom build/os.iso \
    -serial stdio -display none -no-reboot -m 512 -device qemu-xhci \
    -drive file="$IMG",format=raw,if=none,id=d0 -device ahci,id=ahci \
    -device ide-hd,drive=d0,bus=ahci.0 > "$S" 2>&1 & QP=$!; }
waitfor(){ # $1=token $2=tries(2s each)
  for _ in $(seq 1 "$2"); do grep -qF "$1" "$S" 2>/dev/null && return 0; kill -0 "$QP" 2>/dev/null||return 1; sleep 2; done; return 1; }

killq; sleep 1
dd if=/dev/zero of="$IMG" bs=1M count=256 status=none      # blank target

# --- phase 1: author ---
make iso INIT_SCRIPT=user-bin/m2a-init.sh > build/m2a-iso1.log 2>&1 || { echo TEST_FAIL_ISO1; tail -20 build/m2a-iso1.log; exit 1; }
boot 120; waitfor "mkdisk: ok" 50; R=$?; killq; cp "$S" build/serial.m2a1.log
[ $R -eq 0 ] || { echo TEST_FAIL_AUTHOR; tail -30 build/serial.m2a1.log; exit 1; }

# --- host-verify the authored image (real tools; never write the img) ---
sgdisk -v "$IMG" 2>&1 | grep -qiE "No problems|found" || { echo TEST_FAIL_SGDISK; sgdisk -v "$IMG"; exit 1; }
# sgdisk -i N fields:  "First sector: <lba> (at ..)" / "Partition size: <sectors> sectors (..)"
ELBA=$(sgdisk -i 1 "$IMG" | awk -F': ' '/First sector/{print $2}' | awk '{print $1}')
ESEC=$(sgdisk -i 1 "$IMG" | awk '/Partition size/{print $3}')
DLBA=$(sgdisk -i 2 "$IMG" | awk -F': ' '/First sector/{print $2}' | awk '{print $1}')
DSEC=$(sgdisk -i 2 "$IMG" | awk '/Partition size/{print $3}')
dd if="$IMG" of=build/m2a-esp.img bs=512 skip="$ELBA" count="$ESEC" status=none
dd if="$IMG" of=build/m2a-data.img bs=512 skip="$DLBA" count="$DSEC" status=none
fsck.fat -n build/m2a-esp.img  > build/m2a-fsck-esp.log  2>&1
fsck.fat -n build/m2a-data.img > build/m2a-fsck-data.log 2>&1
# fsck.fat -n is read-only; "Free cluster summary uninitialized" is benign (we
# don't write FSInfo free counts). A real FAT prints a "<n> files, <a>/<b> clusters"
# summary; corruption prints hard errors. Gate: summary present AND no hard error.
HARD='Dirty bit|free cluster chain|Reclaim|corrupt|invalid|bad|Got [0-9]'
grep -qiE 'files, .*clusters' build/m2a-fsck-esp.log  || { echo TEST_FAIL_FSCK_ESP;  cat build/m2a-fsck-esp.log;  exit 1; }
grep -qiE "$HARD"            build/m2a-fsck-esp.log  && { echo TEST_FAIL_FSCK_ESP;  cat build/m2a-fsck-esp.log;  exit 1; }
grep -qiE 'files, .*clusters' build/m2a-fsck-data.log || { echo TEST_FAIL_FSCK_DATA; cat build/m2a-fsck-data.log; exit 1; }
grep -qiE "$HARD"            build/m2a-fsck-data.log && { echo TEST_FAIL_FSCK_DATA; cat build/m2a-fsck-data.log; exit 1; }
mdir -i build/m2a-esp.img ::/EFI/BOOT > build/m2a-mdir.log 2>&1 || { echo TEST_FAIL_EFIBOOT; cat build/m2a-mdir.log; exit 1; }

# --- phase 2: round-trip (M1 auto-mounts the authored data partition) ---
make iso INIT_SCRIPT=user-bin/m2a-rt-init.sh > build/m2a-iso2.log 2>&1 || { echo TEST_FAIL_ISO2; tail -20 build/m2a-iso2.log; exit 1; }
boot 120; waitfor "m2a-roundtrip-ok" 55; R=$?; killq; cp "$S" build/serial.m2a2.log
[ $R -eq 0 ] || { echo TEST_FAIL_ROUNDTRIP; tail -30 build/serial.m2a2.log; exit 1; }

echo TEST_PASS_M2A
