#!/usr/bin/env bash
# Integration test: SSH password auth (Step 16, follow-up).
#
# Assumes:
#   - build/disk.img already has /passwd seeded via `make passwd-on-disk
#     RUOS_PASSWORD=ruos` (the Makefile run-passwd-test target enforces this).
#
# Boots the ISO headless under QEMU with port-forward 127.0.0.1:2222 -> :22,
# then runs OpenSSH with sshpass forcing PreferredAuthentications=password.
#
# PASS = serial contains `auth ok user=root (password)` AND remote `pwd`
# returned `/`.
#
# Run from the repo root: bash tests/ssh-passwd-test.sh
set -u
cd "$(dirname "$0")/.."

ISO=build/os.iso
DISK=build/disk.img
PORT=2222
SERIAL=build/serial-passwd.log
EXEC=build/passwd-exec.log

command -v sshpass >/dev/null 2>&1 || { echo "FAIL: sshpass not installed (apt install sshpass)"; exit 1; }

for pid in $(pgrep -f 'qemu-system-x86_64'); do kill -9 "$pid" 2>/dev/null || true; done
sleep 1
for _ in $(seq 1 30); do ss -ltn 2>/dev/null | grep -q ":$PORT " && sleep 1 || break; done

rm -f "$SERIAL" "$EXEC"
timeout 60 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom "$ISO" \
  -serial stdio -display none -no-reboot -m 2048 -device qemu-xhci \
  -netdev user,id=net0,hostfwd=tcp:127.0.0.1:$PORT-:22 \
  -device virtio-net-pci,netdev=net0 \
  -drive file="$DISK",format=raw,if=none,id=disk0 \
  -device ahci,id=ahci -device ide-hd,drive=disk0,bus=ahci.0 \
  > "$SERIAL" 2>&1 &
QEMUPID=$!
sleep 15

echo "--- ssh password exec: 'pwd' (sshpass, no key) ---"
SSHPASS="${RUOS_PASSWORD:-ruos}" sshpass -e ssh -T -p "$PORT" \
  -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
  -o PreferredAuthentications=password -o PubkeyAuthentication=no \
  -o ConnectTimeout=5 root@127.0.0.1 pwd </dev/null > "$EXEC" 2>/dev/null || true
echo "exec received ($(wc -c < "$EXEC") bytes):"
cat -v "$EXEC"

sleep 2
kill "$QEMUPID" 2>/dev/null || true
wait "$QEMUPID" 2>/dev/null || true

echo "--- verdict ---"
ok=1
grep -qF "auth ok user=root (password)" "$SERIAL" || { echo "FAIL: no password auth ok in serial"; ok=0; }
{ grep -q '/' "$EXEC" && [ "$(wc -c < "$EXEC")" -ge 1 ]; } \
                                                  || { echo "FAIL: exec output empty/no slash"; ok=0; }
if [ "$ok" -eq 1 ]; then
  echo TEST_PASS_PASSWD
  exit 0
else
  echo TEST_FAIL_PASSWD
  grep -iE "auth|passwd|ssh " "$SERIAL" | tail -15
  exit 1
fi
