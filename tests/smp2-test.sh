#!/usr/bin/env bash
# Integration test: SMP Fase 2 — parallel compute pool. Boots -smp 4, runs
# `smptest` over SSH, asserts speedup >= 1.5x and >= 2 distinct cores.
set -u
cd "$(dirname "$0")/.."
KEY=build/id_ed25519; PORT=2222
for pid in $(pgrep -f 'qemu-system-x86_64'); do kill -9 "$pid" 2>/dev/null || true; done
sleep 1
for _ in $(seq 1 30); do ss -ltn 2>/dev/null | grep -q ":$PORT " && sleep 1 || break; done
cp "$KEY" /tmp/ruos_id && chmod 600 /tmp/ruos_id
rm -f build/serial-smp2.log build/smptest.log
timeout 60 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -boot d -cdrom build/os.iso \
  -serial stdio -display none -no-reboot -m 256 -device qemu-xhci \
  -netdev user,id=net0,hostfwd=tcp:127.0.0.1:$PORT-:22 \
  -device virtio-net-pci,netdev=net0 \
  -drive file=build/disk.img,format=raw,if=none,id=disk0 \
  -device ahci,id=ahci -device ide-hd,drive=disk0,bus=ahci.0 \
  > build/serial-smp2.log 2>&1 &
QEMUPID=$!
sleep 16
( printf 'smptest\n'; sleep 5; printf 'exit\n'; sleep 1 ) | \
  timeout 25 ssh -tt -p "$PORT" -i /tmp/ruos_id \
    -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    -o ConnectTimeout=5 root@127.0.0.1 > build/smptest.log 2>/dev/null || true
sleep 2
kill "$QEMUPID" 2>/dev/null || true; wait "$QEMUPID" 2>/dev/null || true
echo "=== smptest output ==="; tr -d '\r' < build/smptest.log | grep -iE "parallel=|speedup="
line=$(tr -d '\r' < build/smptest.log | grep -oE 'speedup=[0-9]+\.[0-9]+x cores=\[[0-9,]*\]' | head -1)
spd=$(echo "$line" | grep -oE 'speedup=[0-9]+\.[0-9]+' | grep -oE '[0-9]+\.[0-9]+')
ncores=$(echo "$line" | grep -oE 'cores=\[[0-9,]*\]' | grep -oE '[0-9]+' | sort -u | wc -l)
echo "speedup=$spd distinct_cores=$ncores"
if [ -n "$spd" ] && awk "BEGIN{exit !($spd >= 1.5)}" && [ "${ncores:-0}" -ge 2 ]; then
  echo TEST_PASS_SMP2
else
  echo TEST_FAIL_SMP2; tail -20 build/serial-smp2.log; exit 1
fi
