#!/usr/bin/env bash
# THE GOAL GATE — Step 5: SSH alive while the compositor runs on a dedicated core.
#
# Proves: with -smp 4, the compositor is handed off to the GUI core (cpu 1),
# the BSP executor keeps polling I/O (net/usb/ssh), and an SSH session
# authenticates + delivers a shell prompt WHILE the compositor is running.
#
# Before Step 5 this FAILS: the compositor runs inline on the BSP executor
# (exec_worker_task's run_compositor_gate → never returns), starving ssh_serve_task.
#
# PASS = compositor hand-off marker in serial ("compositor handed off to gui core")
#        AND "auth ok" in serial
#        AND interactive prompt "ruos:/$" received by the SSH client
#
# Run from the repo root:  bash tests/ssh-during-gui-test.sh
# (The Makefile target `run-ssh-gui-test` builds the ISO first, then calls this.)
set -u
cd "$(dirname "$0")/.."

ISO=build/os.iso
DISK=build/disk.img
KEY=build/id_ed25519
PORT=2222
SERIAL=build/serial-ssh-gui.log
CLIENT=build/ssh-gui-client.log

# Kill any stray QEMU from prior runs and wait for the port to free up.
for pid in $(pgrep -f 'qemu-system-x86_64'); do kill -9 "$pid" 2>/dev/null || true; done
sleep 1
for _ in $(seq 1 30); do ss -ltn 2>/dev/null | grep -q ":$PORT " && sleep 1 || break; done

# OpenSSH refuses world-readable private keys; stage a 0600 copy on the
# WSL-native filesystem.
cp "$KEY" /tmp/ruos_id && chmod 600 /tmp/ruos_id

rm -f "$SERIAL" "$CLIENT"

# Boot with -smp 4 so a GuiCompositor AP (cpu 1) exists, and forward SSH port.
timeout 120 qemu-system-x86_64 -machine q35 -cpu max -smp 4 -boot d -cdrom "$ISO" \
  -serial stdio -display none -no-reboot -m 2048 -device qemu-xhci \
  -netdev user,id=net0,hostfwd=tcp:127.0.0.1:$PORT-:22 \
  -device virtio-net-pci,netdev=net0 \
  -drive file="$DISK",format=raw,if=none,id=disk0 \
  -device ahci,id=ahci -device ide-hd,drive=disk0,bus=ahci.0 \
  > "$SERIAL" 2>&1 &
QEMUPID=$!

# Wait for the compositor hand-off marker in the serial log.
# This proves Step 5 fired (exec_worker handed off to GUI core) BEFORE we SSH.
# Timeout: 60 s (compositor starts after init.sh, which runs after full boot).
MARKER_FOUND=0
for i in $(seq 1 60); do
  if grep -qF "compositor handed off to gui core" "$SERIAL" 2>/dev/null; then
    MARKER_FOUND=1
    echo "[ssh-gui-test] compositor hand-off marker found after ${i}s"
    break
  fi
  sleep 1
done

if [ "$MARKER_FOUND" -eq 0 ]; then
  echo "[ssh-gui-test] FAIL: compositor hand-off marker not found in serial within 60s"
  kill "$QEMUPID" 2>/dev/null || true
  wait "$QEMUPID" 2>/dev/null || true
  echo "--- serial tail ---"
  tail -30 "$SERIAL"
  echo TEST_FAIL_SSH_GUI_NO_HANDOFF
  exit 1
fi

# Give SSH a moment to be up (the server starts during boot, well before the
# compositor; by the time we see the hand-off marker SSH is definitely listening).
sleep 2

# Interactive SSH: send 'pwd' + 'exit', capture the prompt.
echo "--- ssh -tt interactive shell (compositor running on GUI core) ---"
( printf 'pwd\n'; sleep 4; printf 'exit\n'; sleep 1 ) | \
  timeout 25 ssh -tt -p "$PORT" -i /tmp/ruos_id \
    -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    -o ConnectTimeout=10 root@127.0.0.1 > "$CLIENT" 2>/dev/null || true

echo "interactive client received ($(wc -c < "$CLIENT") bytes):"
cat -v "$CLIENT"

sleep 2
kill "$QEMUPID" 2>/dev/null || true
wait "$QEMUPID" 2>/dev/null || true

echo "--- verdict ---"
ok=1

# 1. The compositor must have been handed off (Step 5 fired).
if grep -qF "compositor handed off to gui core" "$SERIAL"; then
  echo "PASS: compositor handed off to gui core"
else
  echo "FAIL: compositor hand-off marker absent"
  ok=0
fi

# 2. SSH authentication must have succeeded (proves the BSP executor polled ssh_serve_task).
if grep -qF "auth ok" "$SERIAL"; then
  echo "PASS: auth ok in serial"
else
  echo "FAIL: no auth ok in serial (BSP executor may be stuck)"
  ok=0
fi

# 3. The SSH client must have received the shell prompt.
if grep -qF 'ruos:/$' "$CLIENT"; then
  echo "PASS: interactive shell prompt received"
else
  echo "FAIL: no ruos:/\$ prompt (SSH session or shell didn't start)"
  ok=0
fi

if [ "$ok" -eq 1 ]; then
  echo TEST_PASS_SSH
  exit 0
else
  echo TEST_FAIL_SSH
  echo "--- serial tail ---"
  tail -30 "$SERIAL"
  exit 1
fi
