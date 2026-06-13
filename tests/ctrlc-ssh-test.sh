#!/usr/bin/env bash
# Ctrl-C (VINTR) foreground-kill test.
#
# Over an interactive SSH session, start a long-running cooked-mode app
# (`ping -c 100`), let it run, then send Ctrl-C (0x03). The line discipline
# must cooperatively kill the foreground app and return to the shell prompt.
# We prove the prompt returned by issuing `echo CTRLC_OK` afterwards: it only
# runs if ping was actually killed (otherwise ping holds the foreground).
#
# PASS = '^C' echoed AND 'CTRLC_OK' appears AND ping did not run to completion
#        (fewer than ~90 of the 100 pings).
#
# Run from repo root: bash tests/ctrlc-ssh-test.sh
set -u
cd "$(dirname "$0")/.."

ISO=build/os.iso
DISK=build/disk.img
KEY=build/id_ed25519
PORT=2222
SERIAL=build/serial.log
CLIENT=build/ctrlc-ssh-client.log

for pid in $(pgrep -f 'qemu-system-x86_64'); do kill -9 "$pid" 2>/dev/null || true; done
sleep 1
for _ in $(seq 1 30); do ss -ltn 2>/dev/null | grep -q ":$PORT " && sleep 1 || break; done

cp "$KEY" /tmp/ruos_id && chmod 600 /tmp/ruos_id
rm -f "$SERIAL" "$CLIENT"

timeout 70 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom "$ISO" \
  -serial stdio -display none -no-reboot -m 2048 -device qemu-xhci \
  -netdev user,id=net0,hostfwd=tcp:127.0.0.1:$PORT-:22 \
  -device virtio-net-pci,netdev=net0 \
  -drive file="$DISK",format=raw,if=none,id=disk0 \
  -device ahci,id=ahci -device ide-hd,drive=disk0,bus=ahci.0 \
  > "$SERIAL" 2>&1 &
QEMUPID=$!
sleep 15

echo "--- ssh -tt: ping -c 100, then Ctrl-C, then echo CTRLC_OK ---"
# 0x03 = Ctrl-C. printf '\003' sends the raw byte (no newline) so the line
# discipline sees VINTR rather than a buffered line.
( printf 'ping -c 100 10.0.2.2\n'; sleep 4; printf '\003'; sleep 2; \
  printf 'echo CTRLC_OK\n'; sleep 2; printf 'exit\n'; sleep 1 ) | \
  timeout 30 ssh -tt -p "$PORT" -i /tmp/ruos_id \
    -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    -o ConnectTimeout=5 root@127.0.0.1 > "$CLIENT" 2>/dev/null || true

kill -9 "$QEMUPID" 2>/dev/null || true

echo "client bytes: $(wc -c < "$CLIENT")"
fail() { echo "TEST_FAIL_CTRLC_$1"; exit 1; }

grep -qF '^C' "$CLIENT"        || fail NO_CARET_C       # ldisc echoed ^C
grep -qF 'CTRLC_OK' "$CLIENT"  || fail PROMPT_NOT_BACK  # shell ran next command
# Ping must have been interrupted, not completed all 100.
pings=$(grep -acE 'seq=[0-9]+' "$CLIENT")
echo "ping replies seen: $pings (expect well under 100)"
[ "$pings" -lt 90 ] || fail PING_NOT_KILLED

echo TEST_PASS_CTRLC
