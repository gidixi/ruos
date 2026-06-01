#!/usr/bin/env bash
# SSH idle-survival test: connect, sit idle ~25 s (more than two 10 s watchdog
# checks), then run `echo IDLE_ALIVE`. The session must survive — the idle
# watchdog limit is 5 min, so a fresh session is nowhere near it. If the
# watchdog fires early (stale last_activity on claim), the shell EOFs and the
# echo never runs.  Also dumps any watchdog serial line for diagnosis.
set -u
cd "$(dirname "$0")/.."
ISO=build/os.iso; DISK=build/disk.img; KEY=build/id_ed25519
PORT=2222; SERIAL=build/serial.log; CLIENT=build/ssh-idle-client.log

for pid in $(pgrep -f 'qemu-system-x86_64'); do kill -9 "$pid" 2>/dev/null || true; done
sleep 1
for _ in $(seq 1 30); do ss -ltn 2>/dev/null | grep -q ":$PORT " && sleep 1 || break; done
cp "$KEY" /tmp/ruos_id && chmod 600 /tmp/ruos_id
rm -f "$SERIAL" "$CLIENT"

timeout 90 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom "$ISO" \
  -serial stdio -display none -no-reboot -m 512 -device qemu-xhci \
  -netdev user,id=net0,hostfwd=tcp:127.0.0.1:$PORT-:22 \
  -device virtio-net-pci,netdev=net0 \
  -drive file="$DISK",format=raw,if=none,id=disk0 \
  -device ahci,id=ahci -device ide-hd,drive=disk0,bus=ahci.0 \
  > "$SERIAL" 2>&1 &
QEMUPID=$!
sleep 15

echo "--- ssh -tt: connect, idle 25s, then echo IDLE_ALIVE ---"
( sleep 25; printf 'echo IDLE_ALIVE\n'; sleep 2; printf 'exit\n'; sleep 1 ) | \
  timeout 45 ssh -tt -p "$PORT" -i /tmp/ruos_id \
    -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    -o ConnectTimeout=5 root@127.0.0.1 > "$CLIENT" 2>/dev/null || true

kill -9 "$QEMUPID" 2>/dev/null || true
echo "client bytes: $(wc -c < "$CLIENT")"
echo "watchdog serial lines:"; grep -F "watchdog: pair" "$SERIAL" || echo "  (none)"
if grep -qF 'IDLE_ALIVE' "$CLIENT"; then echo TEST_PASS_IDLE; else echo TEST_FAIL_IDLE; fi
