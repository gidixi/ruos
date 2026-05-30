#!/usr/bin/env bash
# Integration test: SSH server (Step 16, Tasks 6-8).
#
# Boots the ISO headless under QEMU with virtio-net + host port-forward, then
# runs the OpenSSH client twice:
#   1. exec (Task 7): `ssh host pwd` with stdin closed — the client sends an
#      early CHANNEL_EOF; the server must still deliver the full output (the
#      sunset handle_eof patch + bridge close-on-exit, see sunset_io.rs).
#   2. interactive (Task 8): `ssh -tt` with stdin held open, runs `pwd`+`exit`.
#
# PASS = real-signature auth in serial AND interactive prompt `ruos:/$`
# received AND exec output not truncated (contains `/`, > 20 bytes).
#
# Run from the repo root: bash tests/ssh-shell-test.sh
set -u
cd "$(dirname "$0")/.."

ISO=build/os.iso
DISK=build/disk.img
KEY=build/id_ed25519
PORT=2222
SERIAL=build/serial.log
CLIENT=build/ssh-client.log

# Kill any stray QEMU from prior runs and wait for the port to free up.
# (pgrep -f is safe here: this script's own cmdline is "bash tests/...", it
#  does not contain the qemu pattern.)
for pid in $(pgrep -f 'qemu-system-x86_64'); do kill -9 "$pid" 2>/dev/null || true; done
sleep 1
for _ in $(seq 1 30); do ss -ltn 2>/dev/null | grep -q ":$PORT " && sleep 1 || break; done

# OpenSSH refuses world-readable private keys; the repo copy lives on a 0777
# DrvFs mount, so stage a 0600 copy on the WSL-native filesystem.
cp "$KEY" /tmp/ruos_id && chmod 600 /tmp/ruos_id

rm -f "$SERIAL" "$CLIENT"
timeout 60 qemu-system-x86_64 -machine q35 -cpu max -boot d -cdrom "$ISO" \
  -serial stdio -display none -no-reboot -m 512 -device qemu-xhci \
  -netdev user,id=net0,hostfwd=tcp:127.0.0.1:$PORT-:22 \
  -device virtio-net-pci,netdev=net0 \
  -drive file="$DISK",format=raw,if=none,id=disk0 \
  -device ahci,id=ahci -device ide-hd,drive=disk0,bus=ahci.0 \
  > "$SERIAL" 2>&1 &
QEMUPID=$!
sleep 15

EXEC=build/ssh-exec.log

# Task 7 — non-interactive exec. The client closes stdin immediately
# (early CHANNEL_EOF); the server must still deliver the command output.
echo "--- ssh exec: 'pwd' (stdin closed) ---"
timeout 20 ssh -T -p "$PORT" -i /tmp/ruos_id \
  -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
  -o ConnectTimeout=5 root@127.0.0.1 pwd </dev/null > "$EXEC" 2>/dev/null || true
echo "exec client received ($(wc -c < "$EXEC") bytes):"
cat -v "$EXEC"

# Task 8 — interactive shell, stdin held open.
echo "--- ssh -tt interactive shell (stdin held open) ---"
( printf 'pwd\n'; sleep 4; printf 'exit\n'; sleep 1 ) | \
  timeout 25 ssh -tt -p "$PORT" -i /tmp/ruos_id \
    -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    -o ConnectTimeout=5 root@127.0.0.1 > "$CLIENT" 2>/dev/null || true
echo "interactive client received ($(wc -c < "$CLIENT") bytes):"
cat -v "$CLIENT"

sleep 2
kill "$QEMUPID" 2>/dev/null || true
wait "$QEMUPID" 2>/dev/null || true

echo "--- verdict ---"
ok=1
grep -qF "auth ok" "$SERIAL"        || { echo "FAIL: no auth ok in serial"; ok=0; }
grep -qF 'ruos:/$' "$CLIENT"        || { echo "FAIL: interactive shell no prompt"; ok=0; }
{ grep -q '/' "$EXEC" && [ "$(wc -c < "$EXEC")" -gt 20 ]; } \
                                    || { echo "FAIL: exec output truncated"; ok=0; }
if [ "$ok" -eq 1 ]; then
  echo TEST_PASS_SSH
  exit 0
else
  echo TEST_FAIL_SSH
  tail -20 "$SERIAL"
  exit 1
fi
