#!/usr/bin/env bash
set -u
cd "$(dirname "$0")/.."
IMG=build/gpt-disk.img
ISO=build/os.iso
SERIAL=build/serial.log
ps -eo pid,comm | awk '/qemu-system/{print $1}' | while read p; do kill -9 "$p" 2>/dev/null||true; done
sleep 1
dd if=/dev/zero of="$IMG" bs=1M count=64 status=none
sgdisk -n 1:2048:+1M -t 1:EF00 -c 1:EFI \
       -n 2:0:0      -t 2:0700 -c 2:ruos-data "$IMG" >/dev/null
DLBA=$(sgdisk -i 2 "$IMG" | awk '/First sector/{print $3}')
TOTSEC=$(( $(stat -c%s "$IMG") / 512 ))
DSECS=$(( TOTSEC - DLBA - 34 ))
KB=$(( DSECS / 2 ))
rm -f build/data.fat
mkfs.vfat -F 32 -C build/data.fat "$KB" >/dev/null
printf 'gpt-persist-ok\n' > build/marker.txt
mcopy -o -i build/data.fat build/marker.txt ::/GPTHELLO.TXT
dd if=build/data.fat of="$IMG" bs=512 seek="$DLBA" conv=notrunc status=none
timeout 120 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom "$ISO" \
  -serial stdio -display none -no-reboot -m 512 -device qemu-xhci \
  -drive file="$IMG",format=raw,if=none,id=disk0 \
  -device ahci,id=ahci -device ide-hd,drive=disk0,bus=ahci.0 > "$SERIAL" 2>&1 &
QP=$!
# Wait for the boot shell to reach the marker cat (smoke.sh battery runs many
# commands first). Poll up to ~100s; bail early once the marker is on serial.
for _ in $(seq 1 50); do
  grep -qF "gpt-persist-ok" "$SERIAL" 2>/dev/null && break
  kill -0 "$QP" 2>/dev/null || break
  sleep 2
done
ps -eo pid,comm | awk '/qemu-system/{print $1}' | while read p; do kill -9 "$p" 2>/dev/null||true; done
grep -qE "gpt: data part lba=" "$SERIAL" || { echo "TEST_FAIL_GPT_PARSE"; tail -20 "$SERIAL"; exit 1; }
grep -qF "gpt-persist-ok" "$SERIAL" || { echo "TEST_FAIL_GPT_READ"; tail -20 "$SERIAL"; exit 1; }
echo TEST_PASS_GPT
