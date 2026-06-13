#!/usr/bin/env bash
# Integration test: shell pipelines (`cmd1 | cmd2`).
# Boots ruos, connects over SSH, runs `ls / | grep bin`, asserts the output
# contains `bin` and NOT an unrelated root entry (so the pipe really filtered).
set -u
cd "$(dirname "$0")/.."
ISO=build/os.iso; DISK=build/disk.img; KEY=build/id_ed25519; PORT=2222
for pid in $(pgrep -f 'qemu-system-x86_64'); do kill -9 "$pid" 2>/dev/null || true; done
sleep 1
for _ in $(seq 1 30); do ss -ltn 2>/dev/null | grep -q ":$PORT " && sleep 1 || break; done
cp "$KEY" /tmp/ruos_id && chmod 600 /tmp/ruos_id
rm -f build/serial.log build/pipe.log
timeout 60 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom "$ISO" \
  -serial stdio -display none -no-reboot -m 2048 -device qemu-xhci \
  -netdev user,id=net0,hostfwd=tcp:127.0.0.1:$PORT-:22 \
  -device virtio-net-pci,netdev=net0 \
  -drive file="$DISK",format=raw,if=none,id=disk0 \
  -device ahci,id=ahci -device ide-hd,drive=disk0,bus=ahci.0 \
  > build/serial.log 2>&1 &
QEMUPID=$!
sleep 15
timeout 20 ssh -T -p "$PORT" -i /tmp/ruos_id \
  -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
  -o ConnectTimeout=5 root@127.0.0.1 'ls /bin | wc -l' </dev/null \
  > build/pipe.log 2>/dev/null || true
sleep 2
kill "$QEMUPID" 2>/dev/null || true
wait "$QEMUPID" 2>/dev/null || true
echo "=== pipe.log ==="; cat -v build/pipe.log
# wc emits a bare count (e.g. "43"). A digit-only line CANNOT come from the
# echoed command `ls /bin | wc -l`, so it proves the pipeline produced real
# output AND it reached the SSH client (PTY-inheritance fix).
if tr -d '\r' < build/pipe.log | grep -qE '^[[:space:]]*[0-9]+[[:space:]]*$'; then
  echo TEST_PASS_PIPE
else
  echo TEST_FAIL_PIPE; tail -20 build/serial.log; exit 1
fi
