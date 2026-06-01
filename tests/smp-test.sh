#!/usr/bin/env bash
# Integration test: SMP Fase 1 — APs brought up to idle on QEMU -smp 4.
# Asserts the BSP enumerated 4 CPUs AND all 3 APs reached online, with no #PF.
set -u
cd "$(dirname "$0")/.."

ISO=build/os.iso
LOG=build/serial-smp.log
DISK=build/disk.img

for pid in $(pgrep -f 'qemu-system-x86_64'); do kill -9 "$pid" 2>/dev/null || true; done
sleep 1
rm -f "$LOG"

# Include AHCI disk if present (mirrors the full boot so init.sh can complete),
# but the KEY assertion fires during the interrupts phase — well before storage.
if [ -f "$DISK" ]; then
  DISK_ARGS="-drive file=$DISK,format=raw,if=none,id=disk0 -device ahci,id=ahci -device ide-hd,drive=disk0,bus=ahci.0"
else
  DISK_ARGS=""
fi

# shellcheck disable=SC2086
timeout 60 qemu-system-x86_64 \
  -machine q35 -cpu max -smp 4 -boot d -cdrom "$ISO" \
  -serial stdio -display none -no-reboot -m 256 \
  $DISK_ARGS \
  > "$LOG" 2>&1 || true

echo "=== smp markers ==="
grep -iE "CPU\(s\) found|APs online|#PF|panic|init.sh complete" "$LOG" || true

if grep -qE "smp +3/3 APs online" "$LOG" \
   && ! grep -qE "#PF|KERNEL PANIC" "$LOG"; then
  echo TEST_PASS_SMP
else
  echo TEST_FAIL_SMP
  tail -25 "$LOG"
  exit 1
fi
