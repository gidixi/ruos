#!/usr/bin/env bash
# Integration test: SSH password auth with NO disk attached.
#
# Boots the ISO under QEMU without any SATA drive (no /mnt, no /passwd,
# no /auth.key, no /host.key). Verifies that:
#   - the kernel still boots and brings the SSH server up
#   - the host key is ephemeral (logged warning) but the server accepts
#     connections
#   - password auth falls back to the compile-time default (`ruos` unless
#     overridden via `RUOS_DEFAULT_PASSWORD` at build time)
#
# Run from the repo root: bash tests/ssh-passwd-diskless-test.sh
set -u
cd "$(dirname "$0")/.."

ISO=build/os.iso
PORT=2222
SERIAL=build/serial-diskless.log
EXEC=build/diskless-exec.log

command -v sshpass >/dev/null 2>&1 || { echo "FAIL: sshpass missing"; exit 1; }

for pid in $(pgrep -f 'qemu-system-x86_64'); do kill -9 "$pid" 2>/dev/null || true; done
sleep 1
for _ in $(seq 1 30); do ss -ltn 2>/dev/null | grep -q ":$PORT " && sleep 1 || break; done

rm -f "$SERIAL" "$EXEC"
# Same QEMU command as the other tests, MINUS the `-drive`/`-device ahci`
# pair: no SATA disk attached.
timeout 60 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom "$ISO" \
  -serial stdio -display none -no-reboot -m 512 -device qemu-xhci \
  -netdev user,id=net0,hostfwd=tcp:127.0.0.1:$PORT-:22 \
  -device virtio-net-pci,netdev=net0 \
  > "$SERIAL" 2>&1 &
QEMUPID=$!
sleep 15

echo "--- ssh password exec: 'pwd' (no disk, default password) ---"
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
grep -qF "password fallback to built-in default" "$SERIAL" \
                                      || { echo "FAIL: no built-in default fallback log"; ok=0; }
grep -qF "auth ok user=root (password)" "$SERIAL" \
                                      || { echo "FAIL: no password auth ok in serial"; ok=0; }
{ grep -q '/' "$EXEC" && [ "$(wc -c < "$EXEC")" -ge 1 ]; } \
                                      || { echo "FAIL: exec output empty/no slash"; ok=0; }
if [ "$ok" -eq 1 ]; then
  echo TEST_PASS_PASSWD_DISKLESS
  exit 0
else
  echo TEST_FAIL_PASSWD_DISKLESS
  grep -iE "auth|passwd|ssh |fat|mnt" "$SERIAL" | tail -20
  exit 1
fi
